//! The same account sources as t3code: Codex app-server, Claude's SDK control
//! protocol and Cursor's current-period dashboard endpoint. No agent turn runs.
use super::{MAX_WINDOWS, Provider, Unavailable, Window, timestamp};
use neptune_model::AgentKind;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const CURSOR_ENDPOINT: &str = "https://api2.cursor.sh";
const MAX_FRAME: u64 = 1024 * 1024;
const MAX_OUTPUT: u64 = 4 * MAX_FRAME;
const TIMEOUT: Duration = Duration::from_secs(20);
type Result<T> = std::result::Result<T, Unavailable>;

pub(super) fn read(
    provider: Provider,
    keychain: bool,
    stop: &AtomicBool,
) -> Result<(Vec<Window>, Option<String>)> {
    if provider == Provider::Cursor {
        return cursor(keychain, stop).map(|windows| (windows, None));
    }
    let kind = if provider == Provider::Codex {
        AgentKind::Codex
    } else {
        AgentKind::Claude
    };
    let executable =
        super::super::project_lead::executable(kind, stop).ok_or(Unavailable::NotInstalled)?;
    let mut command = Command::new(executable);
    if provider == Provider::Codex {
        command.arg("app-server");
    } else {
        command
            .args([
                "--print",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--verbose",
                "--no-session-persistence",
                "--tools",
                "",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--settings",
                "{\"disableAllHooks\":true}",
                "--setting-sources",
                "",
            ])
            .env("CLAUDE_CODE_ENTRYPOINT", "sdk-ts")
            .env("ENABLE_CLAUDEAI_MCP_SERVERS", "false")
            .env("CLAUDE_CODE_AUTO_CONNECT_IDE", "0")
            .env("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL", "1")
            .env_remove("FORCE_CODE_TERMINAL");
        command.env_remove("CLAUDECODE");
    }
    // Account probes have no project directory, prompt, tools or saved session.
    if let Some(home) = directories::BaseDirs::new() {
        command.current_dir(home.home_dir());
    }
    let response = rpc(&mut command, provider, stop, TIMEOUT)?;
    let windows = match provider {
        Provider::Codex => codex_windows(&response),
        _ => claude_windows(&response),
    };
    let windows = windows?;
    let plan = match provider {
        Provider::Codex => codex_plan(&response),
        _ => response["neptune_plan"].as_str(),
    }
    .map(|text| text.chars().take(48).collect());
    Ok((windows, plan))
}

/// A bounded pipe reader keeps credentials in memory and applies backpressure.
/// The worker cancels the owned process group and drops the mailbox on teardown.
struct Probe {
    child: Child,
    output: Option<mpsc::Receiver<Result<Vec<u8>>>>,
    reader: Option<thread::JoinHandle<()>>,
    reaped: bool,
}
impl Probe {
    fn spawn(command: &mut Command) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stderr(Stdio::null())
            .stdout(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let child = command.spawn().map_err(|_| Unavailable::Failed)?;
        let (send, receive) = mpsc::sync_channel(4);
        let mut probe = Self {
            child,
            output: Some(receive),
            reader: None,
            reaped: false,
        };
        let mut stdout = probe.child.stdout.take().ok_or(Unavailable::Failed)?;
        probe.reader = Some(
            thread::Builder::new()
                .name("neptune-usage-output".into())
                .spawn(move || {
                    loop {
                        let mut bytes = vec![0; 64 * 1024];
                        let result = match stdout.read(&mut bytes) {
                            Ok(0) => return,
                            Ok(read) => {
                                bytes.truncate(read);
                                Ok(bytes)
                            }
                            Err(_) => Err(Unavailable::Failed),
                        };
                        let failed = result.is_err();
                        if send.send(result).is_err() || failed {
                            return;
                        }
                    }
                })
                .map_err(|_| Unavailable::Failed)?,
        );
        Ok(probe)
    }
    fn send(&mut self, value: Value) -> Result<()> {
        let input = self.child.stdin.as_mut().ok_or(Unavailable::Failed)?;
        writeln!(input, "{value}").map_err(|_| Unavailable::Failed)
    }
    /// False after EOF, once every queued byte has been taken.
    fn bytes(&mut self, offset: &mut u64, buffer: &mut Vec<u8>) -> Result<bool> {
        let output = self.output.as_ref().ok_or(Unavailable::Failed)?;
        let bytes = match output.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => result?,
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(true),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(false),
        };
        *offset += bytes.len() as u64;
        buffer.extend_from_slice(&bytes);
        if *offset > MAX_OUTPUT || buffer.len() as u64 > MAX_FRAME {
            return Err(Unavailable::Failed);
        }
        Ok(true)
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        self.output = None;
        // The root remains unreaped until here, so its group id cannot have
        // been recycled. Only processes created by this probe are signalled.
        if !self.reaped {
            super::super::project_lead::signal(self.child.id(), "-KILL");
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let deadline = Instant::now() + Duration::from_millis(100);
        while self
            .reader
            .as_ref()
            .is_some_and(|reader| !reader.is_finished())
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        if self
            .reader
            .as_ref()
            .is_some_and(|reader| reader.is_finished())
            && let Some(reader) = self.reader.take()
        {
            let _ = reader.join();
        }
    }
}

fn rpc(
    command: &mut Command,
    provider: Provider,
    stop: &AtomicBool,
    timeout: Duration,
) -> Result<Value> {
    let mut probe = Probe::spawn(command)?;
    let codex = provider == Provider::Codex;
    probe.send(if codex {
        json!({"id":"usage-init","method":"initialize","params":{"clientInfo":{"name":"neptune_usage","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}})
    } else {
        json!({"type":"control_request","request_id":"usage-init","request":{"subtype":"initialize"}})
    })?;
    let deadline = Instant::now() + timeout;
    let mut initialized = false;
    let mut plan = None;
    let mut buffer = Vec::new();
    let mut offset = 0;
    loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(Unavailable::Failed);
        }
        let alive = probe.bytes(&mut offset, &mut buffer)?;
        while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            let frame: Value =
                serde_json::from_slice(&buffer[..end]).map_err(|_| Unavailable::Failed)?;
            buffer.drain(..=end);
            let id = if codex {
                frame["id"].as_str()
            } else {
                frame["response"]["request_id"].as_str()
            };
            if id == Some("usage-init") && !initialized {
                if frame.get("error").is_some() || frame["response"]["subtype"] == "error" {
                    return Err(Unavailable::Failed);
                }
                initialized = true;
                plan = frame["response"]["response"]["account"]["subscriptionType"]
                    .as_str()
                    .map(str::to_owned);
                if codex {
                    probe.send(json!({"method":"initialized"}))?;
                }
                probe.send(if codex {
                    json!({"id":"usage-read","method":"account/rateLimits/read","params":null})
                } else {
                    // The SDK option skipBehaviors is snake_case on the wire.
                    json!({"type":"control_request","request_id":"usage-read","request":{"subtype":"get_usage","skip_behaviors":true}})
                })?;
            } else if id == Some("usage-read") && initialized {
                if frame.get("error").is_some() {
                    return Err(rpc_failure(&frame["error"]));
                }
                if frame["response"]["subtype"] == "error" {
                    return Err(Unavailable::Unsupported);
                }
                let mut response = if codex {
                    frame["result"].clone()
                } else {
                    frame["response"]["response"].clone()
                };
                if !response.is_object() {
                    return Err(Unavailable::Failed);
                }
                if let Some(plan) = plan {
                    response["neptune_plan"] = plan.into();
                }
                return Ok(response);
            }
        }
        if !alive {
            return Err(Unavailable::Failed);
        }
    }
}

fn rpc_failure(error: &Value) -> Unavailable {
    if error["code"] == -32601 {
        return Unavailable::Unsupported;
    }
    let message = error["message"].as_str().unwrap_or("").to_ascii_lowercase();
    if [
        "not logged in",
        "not signed in",
        "unauthorized",
        "requires chatgpt authentication",
    ]
    .iter()
    .any(|text| message.contains(text))
    {
        Unavailable::SignIn
    } else {
        Unavailable::Failed
    }
}

fn percent(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|number| number.is_finite())
        .map(|number| number.clamp(0.0, 100.0))
}
fn main_codex_snapshot(response: &Value) -> &Value {
    response["rateLimitsByLimitId"]
        .get("codex")
        .unwrap_or(&response["rateLimits"])
}

/// Readable names from Codex's `KnownPlan::display_name`, not wire identifiers:
/// https://github.com/openai/codex/blob/main/codex-rs/protocol/src/auth.rs
fn codex_plan(response: &Value) -> Option<&'static str> {
    Some(match main_codex_snapshot(response)["planType"].as_str()? {
        "free" => "Free",
        "go" => "Go",
        "plus" => "Plus",
        "prolite" => "Pro",
        "pro" => "Pro (More)",
        "promax" => "Pro (Max)",
        "team" => "Team",
        "self_serve_business_prolite" => "Self Serve Business ProLite",
        "self_serve_business_usage_based" => "Self Serve Business Usage Based",
        "business" => "Business",
        "ent26" | "enterprise" | "hc" => "Enterprise",
        "enterprise_cbp_automation" => "Enterprise (Automation)",
        "enterprise_cbp_usage_based" => "Enterprise CBP Usage Based",
        "education" | "edu" => "Edu",
        "edu_plus" => "Edu Plus",
        "edu_pro" => "Edu Pro",
        // A new identifier must not become an invented or opaque plan label.
        _ => return None,
    })
}
fn codex_windows(response: &Value) -> Result<Vec<Window>> {
    let snapshot = main_codex_snapshot(response);
    if snapshot["limitId"].as_str().is_some_and(|id| id != "codex") {
        return Err(Unavailable::Unsupported);
    }
    let monthly = matches!(snapshot["planType"].as_str(), Some("free" | "go"));
    let mut windows = Vec::new();
    for (position, fallback) in [
        ("primary", if monthly { 43200 } else { 300 }),
        ("secondary", 10080),
    ] {
        let raw = &snapshot[position];
        let Some(used_percent) = percent(&raw["usedPercent"]) else {
            continue;
        };
        let duration = raw["windowDurationMins"].as_u64().unwrap_or(fallback);
        let label = if duration >= 43200 {
            "Monthly"
        } else if duration >= 10080 {
            "Weekly"
        } else {
            "Session"
        };
        windows.push(Window {
            label: label.into(),
            used_percent,
            resets_at: raw["resetsAt"].as_u64().filter(|value| *value > 0),
        });
    }
    available(windows)
}
fn claude_windows(response: &Value) -> Result<Vec<Window>> {
    if response["rate_limits_available"] != true {
        return Err(Unavailable::Unsupported);
    }
    let limits = &response["rate_limits"];
    let mut windows = Vec::new();
    for (id, label) in [("five_hour", "Session"), ("seven_day", "Weekly")] {
        if let Some(used_percent) = percent(&limits[id]["utilization"]) {
            windows.push(Window {
                label: label.into(),
                used_percent,
                resets_at: limits[id]["resets_at"].as_str().and_then(timestamp),
            });
        }
    }
    if let Some(scoped) = limits["model_scoped"].as_array() {
        for entry in scoped.iter().take(MAX_WINDOWS - windows.len()) {
            if let (Some(name), Some(used_percent)) = (
                entry["display_name"].as_str(),
                percent(&entry["utilization"]),
            ) {
                windows.push(Window {
                    label: format!("Weekly · {}", name.chars().take(48).collect::<String>()),
                    used_percent,
                    resets_at: entry["resets_at"].as_str().and_then(timestamp),
                });
            }
        }
    }
    available(windows)
}
fn available(windows: Vec<Window>) -> Result<Vec<Window>> {
    if windows.is_empty() {
        Err(Unavailable::Unsupported)
    } else {
        Ok(windows)
    }
}

#[derive(Default)]
struct CursorLogin {
    token: Option<String>,
    api_key: Option<String>,
    store: Option<String>,
    endpoint: Option<String>,
    home: PathBuf,
    config: Option<PathBuf>,
    appdata: Option<PathBuf>,
}
impl CursorLogin {
    fn from_env() -> Self {
        let text = |name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(|value| value.trim().to_owned())
        };
        Self {
            token: text("CURSOR_AUTH_TOKEN"),
            api_key: text("CURSOR_API_KEY"),
            store: text("AGENT_CLI_CREDENTIAL_STORE"),
            endpoint: text("CURSOR_API_ENDPOINT"),
            home: directories::BaseDirs::new()
                .map(|dirs| dirs.home_dir().to_owned())
                .unwrap_or_default(),
            config: std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            appdata: std::env::var_os("APPDATA").map(PathBuf::from),
        }
    }
    fn credentials(&self) -> PathBuf {
        if cfg!(windows) {
            self.appdata
                .clone()
                .unwrap_or_else(|| self.home.join("AppData/Roaming"))
                .join("Cursor/auth.json")
        } else if cfg!(target_os = "macos") {
            self.home.join(".cursor/auth.json")
        } else {
            self.config
                .clone()
                .unwrap_or_else(|| self.home.join(".config"))
                .join("cursor/auth.json")
        }
    }
    fn token(&self, keychain: bool, stop: &AtomicBool) -> Result<String> {
        if let Some(token) = &self.token {
            return Ok(token.clone());
        }
        // Do not show a saved account's quota for an explicitly different API-key login.
        if self.api_key.is_some() || self.store.as_deref() == Some("memory") {
            return Err(Unavailable::Unsupported);
        }
        if cfg!(target_os = "macos") && self.store.as_deref() != Some("file") {
            if !keychain {
                return Err(Unavailable::Keychain);
            }
            if self
                .endpoint
                .as_deref()
                .is_some_and(|endpoint| endpoint.trim_end_matches('/') != CURSOR_ENDPOINT)
            {
                return Err(Unavailable::Unsupported);
            }
            return keychain_token(stop);
        }
        let file = match File::open(self.credentials()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Unavailable::SignIn);
            }
            Err(_) => return Err(Unavailable::Failed),
        };
        let mut bytes = Vec::new();
        file.take(MAX_FRAME + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Unavailable::Failed)?;
        if bytes.len() as u64 > MAX_FRAME {
            return Err(Unavailable::Failed);
        }
        let credentials: Value = serde_json::from_slice(&bytes).map_err(|_| Unavailable::Failed)?;
        credentials["accessToken"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(|text| text.trim().to_owned())
            .ok_or(Unavailable::SignIn)
    }
}

#[cfg(not(target_os = "macos"))]
fn keychain_token(_stop: &AtomicBool) -> Result<String> {
    Err(Unavailable::Unsupported)
}
#[cfg(target_os = "macos")]
fn keychain_token(stop: &AtomicBool) -> Result<String> {
    let mut command = Command::new("/usr/bin/security");
    command.args([
        "find-generic-password",
        "-s",
        "cursor-access-token",
        "-a",
        "cursor-user",
        "-w",
    ]);
    let mut probe = Probe::spawn(&mut command)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut offset = 0;
    let mut bytes = Vec::new();
    loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(Unavailable::Failed);
        }
        if !probe.bytes(&mut offset, &mut bytes)? {
            let status = probe.child.wait().map_err(|_| Unavailable::Failed)?;
            probe.reaped = true;
            if !status.success() {
                return Err(Unavailable::SignIn);
            }
            return String::from_utf8(bytes)
                .ok()
                .filter(|text| !text.trim().is_empty())
                .map(|text| text.trim().to_owned())
                .ok_or(Unavailable::SignIn);
        }
    }
}

fn cursor(keychain: bool, stop: &AtomicBool) -> Result<Vec<Window>> {
    let login = CursorLogin::from_env();
    let token = login.token(keychain, stop)?;
    if stop.load(Ordering::Acquire) {
        return Err(Unavailable::Failed);
    }
    let endpoint = login
        .endpoint
        .as_deref()
        .unwrap_or(CURSOR_ENDPOINT)
        .trim_end_matches('/');
    // Redirects may send the login to another server; never follow them.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let response = agent
        .post(format!(
            "{endpoint}/aiserver.v1.DashboardService/GetCurrentPeriodUsage"
        ))
        .header("Authorization", &format!("Bearer {token}"))
        .header("connect-protocol-version", "1")
        .header("x-cursor-client-type", "cli")
        .header("Content-Type", "application/json")
        .send("{}");
    let mut response = match response {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => return Err(Unavailable::SignIn),
        Err(_) => return Err(Unavailable::Failed),
    };
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_FRAME)
        .read_to_vec()
        .map_err(|_| Unavailable::Failed)?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| Unavailable::Failed)?;
    cursor_windows(&body)
}
fn cursor_windows(body: &Value) -> Result<Vec<Window>> {
    let reset = body["billingCycleEnd"]
        .as_u64()
        .or_else(|| body["billingCycleEnd"].as_str()?.parse().ok())
        .filter(|value| *value > 0)
        .map(|value| value / 1000);
    let windows = [
        ("totalPercentUsed", "Monthly"),
        ("autoPercentUsed", "Auto"),
        ("apiPercentUsed", "API"),
    ]
    .into_iter()
    .filter_map(|(id, label)| {
        Some(Window {
            label: label.into(),
            used_percent: percent(&body["planUsage"][id])?,
            resets_at: reset,
        })
    })
    .collect();
    available(windows)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_plan_uses_readable_names_from_the_main_usage_bucket() {
        for (raw, label) in [
            ("prolite", Some("Pro")),
            ("pro", Some("Pro (More)")),
            ("promax", Some("Pro (Max)")),
            ("plus", Some("Plus")),
            ("free", Some("Free")),
            ("business", Some("Business")),
            ("ent26", Some("Enterprise")),
            ("education", Some("Edu")),
            ("unknown", None),
            ("future_plan_id", None),
        ] {
            let response = json!({
                "rateLimits": {"planType": "plus"},
                "rateLimitsByLimitId": {"codex": {"planType": raw}},
            });
            assert_eq!(codex_plan(&response), label, "planType {raw}");
        }
        assert_eq!(
            codex_plan(&json!({"rateLimits": {"planType": "prolite"}})),
            Some("Pro")
        );
        assert_eq!(codex_plan(&json!({})), None);
    }

    #[test]
    fn codex_uses_main_bucket_and_plan_duration_fallback() {
        let body = json!({"rateLimits":{"limitId":"spark","primary":{"usedPercent":99}},"rateLimitsByLimitId":{"codex":{"planType":"free","primary":{"usedPercent":15,"resetsAt":42}}}});
        assert_eq!(
            codex_windows(&body).unwrap()[0],
            Window {
                label: "Monthly".into(),
                used_percent: 15.0,
                resets_at: Some(42)
            }
        );
        assert!(
            codex_windows(&json!({"rateLimits":{"limitId":"spark","primary":{"usedPercent":99}}}))
                .is_err()
        );
    }
    #[test]
    fn claude_percentages_are_not_fractions_and_scoped_windows_are_bounded() {
        let body = json!({"rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":23.5,"resets_at":"2026-10-09T12:30:00Z"},"seven_day":{"utilization":120},"model_scoped":vec![json!({"display_name":"Opus","utilization":50}); 100]}});
        let windows = claude_windows(&body).unwrap();
        assert_eq!(windows.len(), MAX_WINDOWS);
        assert_eq!(windows[0].used_percent, 23.5);
        assert_eq!(windows[1].used_percent, 100.0);
        assert_eq!(windows[2].label, "Weekly · Opus");
    }
    #[test]
    fn cursor_uses_dashboard_percentages_including_bonus_usage() {
        let body = json!({"billingCycleEnd":"1791549000000","planUsage":{"totalPercentUsed":31.2,"autoPercentUsed":0,"apiPercentUsed":44.8}});
        let windows = cursor_windows(&body).unwrap();
        assert_eq!(
            windows
                .iter()
                .map(|window| window.used_percent)
                .collect::<Vec<_>>(),
            vec![31.2, 0.0, 44.8]
        );
        assert_eq!(windows[0].resets_at, Some(1791549000));
        assert!(cursor_windows(&json!({})).is_err());
    }
    #[test]
    fn cursor_api_key_never_falls_back_to_another_accounts_file() {
        let login = CursorLogin {
            api_key: Some("api-key".into()),
            ..Default::default()
        };
        assert_eq!(
            login.token(false, &AtomicBool::new(false)),
            Err(Unavailable::Unsupported)
        );
        let login = CursorLogin {
            token: Some("auth-token".into()),
            ..login
        };
        assert_eq!(
            login.token(false, &AtomicBool::new(false)).unwrap(),
            "auth-token"
        );
    }
    #[test]
    fn cursor_reads_the_platform_login_without_retaining_credentials() {
        let root = tempfile::tempdir().unwrap();
        let login = CursorLogin {
            home: root.path().into(),
            config: Some(root.path().into()),
            appdata: Some(root.path().into()),
            store: Some("file".into()),
            ..Default::default()
        };
        let path = login.credentials();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"accessToken":"saved-login"}"#).unwrap();
        assert_eq!(
            login.token(false, &AtomicBool::new(false)).unwrap(),
            "saved-login"
        );
        std::fs::write(&path, b"broken credentials").unwrap();
        assert_eq!(
            login.token(false, &AtomicBool::new(false)),
            Err(Unavailable::Failed)
        );
    }
    #[test]
    fn protocol_failures_distinguish_logout_from_transient_errors() {
        assert_eq!(
            rpc_failure(&json!({"code":-32601})),
            Unavailable::Unsupported
        );
        assert_eq!(
            rpc_failure(&json!({"message":"Not signed in"})),
            Unavailable::SignIn
        );
        assert_eq!(
            rpc_failure(&json!({"message":"Service unavailable"})),
            Unavailable::Failed
        );
    }
    #[cfg(unix)]
    #[test]
    fn claude_protocol_reads_usage_without_behaviors_or_a_prompt() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c",r#"read init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"usage-init","response":{"account":{"subscriptionType":"max"}}}}'
read usage
case "$usage" in *get_usage*) ;; *) exit 1;; esac
case "$usage" in *skip_behaviors*) ;; *) exit 2;; esac
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"usage-read","response":{"rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":25}}}}}'
read no_turn"#]);
        let result = rpc(
            &mut command,
            Provider::Claude,
            &AtomicBool::new(false),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(claude_windows(&result).unwrap()[0].used_percent, 25.0);
        assert_eq!(result["neptune_plan"], "max");
    }
    #[cfg(unix)]
    #[test]
    fn oversized_output_is_bounded_and_probe_is_reaped() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec head -c 2097152 /dev/zero"]);
        assert_eq!(
            rpc(
                &mut command,
                Provider::Codex,
                &AtomicBool::new(false),
                Duration::from_secs(2)
            ),
            Err(Unavailable::Failed)
        );
    }
    #[cfg(unix)]
    #[test]
    fn codex_protocol_initializes_then_reads_without_a_turn() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            r#"read init
case "$init" in *initialize*) ;; *) exit 1;; esac
printf '%s\n' '{"id":"usage-init","result":{}}'
read initialized
read usage
case "$usage" in *account/rateLimits/read*) ;; *) exit 2;; esac
printf '%s\n' '{"id":"usage-read","result":{"rateLimits":{"primary":{"usedPercent":12}}}}'
read no_turn
exit 3"#,
        ]);
        let result = rpc(
            &mut command,
            Provider::Codex,
            &AtomicBool::new(false),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(codex_windows(&result).unwrap()[0].used_percent, 12.0);
    }
    #[cfg(unix)]
    #[test]
    fn a_silent_cli_times_out_and_is_reaped() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "read init; read next"]);
        assert_eq!(
            rpc(
                &mut command,
                Provider::Claude,
                &AtomicBool::new(false),
                Duration::from_millis(100)
            ),
            Err(Unavailable::Failed)
        );
    }
}
