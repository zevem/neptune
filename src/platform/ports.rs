//! Process-owned TCP listeners. All filesystem and OS queries run on a worker.
//! No command lines, environment, terminal text or network traffic are read.
#[cfg(target_os = "linux")]
use std::collections::VecDeque;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const MAX_PORTS: usize = 16;
pub const UNIX_PROBE: &str = include_str!("ports-probe.sh");

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Listener {
    pub port: u16,
    /// Wildcard bindings are normalized to the corresponding loopback address.
    pub address: IpAddr,
}
impl Listener {
    pub fn label(self) -> String {
        if self.address == IpAddr::V4(Ipv4Addr::LOCALHOST)
            || self.address == IpAddr::V6(Ipv6Addr::LOCALHOST)
        {
            format!("localhost:{}", self.port)
        } else {
            self.authority()
        }
    }
    pub fn authority(self) -> String {
        match self.address {
            IpAddr::V4(address) => format!("{address}:{}", self.port),
            IpAddr::V6(address) => format!("[{address}]:{}", self.port),
        }
    }
    pub fn url(self) -> String {
        // Browser origins must match the localhost name shown on loopback chips.
        // SSH forwarding still uses authority() to reach the actual bind address.
        format!("http://{}/", self.label())
    }
}

fn listener(address: IpAddr, port: u16) -> Option<Listener> {
    (port != 0).then_some(Listener {
        port,
        address: match address {
            IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
            IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
            address => address,
        },
    })
}

fn proc_address(text: &str, little_endian: bool) -> Option<Listener> {
    let (address, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word_bytes = |word: u32| {
        if little_endian {
            word.to_le_bytes()
        } else {
            word.to_be_bytes()
        }
    };
    let address = match address.len() {
        8 => IpAddr::V4(Ipv4Addr::from(word_bytes(
            u32::from_str_radix(address, 16).ok()?,
        ))),
        32 => {
            let mut bytes = [0; 16];
            for (index, word) in bytes.chunks_exact_mut(4).enumerate() {
                word.copy_from_slice(&word_bytes(
                    u32::from_str_radix(&address[index * 8..index * 8 + 8], 16).ok()?,
                ));
            }
            IpAddr::V6(Ipv6Addr::from(bytes))
        }
        _ => return None,
    };
    listener(address, port)
}

fn compact(listeners: impl IntoIterator<Item = Listener>) -> Vec<Listener> {
    // Prefer IPv4 when a server binds both families on the same port.
    let mut ports = BTreeMap::new();
    let sorted: BTreeSet<_> = listeners.into_iter().collect();
    for item in sorted {
        if ports.len() >= MAX_PORTS {
            break;
        }
        ports.entry(item.port).or_insert(item);
    }
    ports.into_values().collect()
}

pub fn parse_probe(text: &str) -> Vec<Listener> {
    let little_endian = match text.lines().next() {
        Some("endian big") => false,
        Some("endian little") => true,
        _ => cfg!(target_endian = "little"),
    };
    compact(text.lines().take(8192).filter_map(|line| {
        let mut fields = line.split_whitespace();
        match fields.next()? {
            "tcp" => proc_address(fields.next()?, little_endian),
            "ip" => listener(fields.next()?.parse().ok()?, fields.next()?.parse().ok()?),
            "endpoint" => {
                let endpoint = fields.next()?;
                let (host, port) = endpoint.rsplit_once(':')?;
                let host = host.trim_matches(['[', ']']);
                listener(
                    if host == "*" {
                        Ipv4Addr::LOCALHOST.into()
                    } else {
                        host.parse().ok()?
                    },
                    port.parse().ok()?,
                )
            }
            _ => None,
        }
    }))
}

/// Run a metadata-only helper with a deadline and bounded output. Output goes
/// to a task-owned file so a full pipe never stalls process teardown.
pub fn run(command: &mut Command) -> Result<String, String> {
    let output = tempfile::tempfile().map_err(|_| "Could not prepare port discovery".to_owned())?;
    command.stdin(Stdio::null()).stderr(Stdio::null()).stdout(
        output
            .try_clone()
            .map_err(|_| "Could not prepare port discovery".to_owned())?,
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start port discovery".to_owned())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None)
                if Instant::now() < deadline
                    && output.metadata().is_ok_and(|m| m.len() <= 65_536) =>
            {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                #[cfg(unix)]
                {
                    let _ = Command::new("kill")
                        .args(["-KILL", "--", &format!("-{}", child.id())])
                        .status();
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err("Port discovery timed out or exceeded its limit".into());
            }
        }
    };
    if !output
        .metadata()
        .is_ok_and(|metadata| metadata.len() <= 65_536)
    {
        return Err("Port discovery exceeded its output limit".into());
    }
    if !success {
        return Err("Could not inspect ports or change forwarding. Check the SSH connection and available port.".into());
    }
    use std::io::{Seek, SeekFrom};
    let mut output = output;
    output
        .seek(SeekFrom::Start(0))
        .map_err(|_| "Could not read port discovery".to_owned())?;
    let mut text = String::new();
    output
        .take(65_536)
        .read_to_string(&mut text)
        .map_err(|_| "Could not read port discovery".to_owned())?;
    Ok(text)
}

#[cfg(target_os = "linux")]
pub fn discover(pid: u32) -> Result<Vec<Listener>, String> {
    let started = Instant::now();
    let mut pending = VecDeque::from([pid]);
    let mut seen = BTreeSet::new();
    let mut sockets = BTreeSet::new();
    let mut descriptors = 0;
    while let Some(pid) = pending.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        if seen.len() > 2048 || started.elapsed() > Duration::from_millis(250) {
            return Err("Port discovery reached its process limit".into());
        }
        if let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) {
            for task in tasks.take(64).flatten() {
                if let Ok(file) = std::fs::File::open(task.path().join("children")) {
                    let mut children = String::new();
                    if file.take(32_768).read_to_string(&mut children).is_ok() {
                        pending.extend(
                            children
                                .split_whitespace()
                                .filter_map(|pid| pid.parse::<u32>().ok())
                                .take(2048 - pending.len().min(2048)),
                        );
                    }
                }
            }
        }
        if let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) {
            for fd in fds.flatten() {
                descriptors += 1;
                if descriptors > 8192 || started.elapsed() > Duration::from_millis(250) {
                    return Err("Port discovery reached its descriptor limit".into());
                }
                if let Ok(link) = std::fs::read_link(fd.path()) {
                    let text = link.to_string_lossy();
                    if let Some(inode) = text
                        .strip_prefix("socket:[")
                        .and_then(|s| s.strip_suffix(']'))
                        .and_then(|s| s.parse::<u64>().ok())
                    {
                        sockets.insert(inode);
                    }
                }
            }
        }
    }
    let mut listeners = Vec::new();
    for family in ["tcp", "tcp6"] {
        let file = std::fs::File::open(format!("/proc/{pid}/net/{family}"))
            .map_err(|_| "Could not read listening sockets".to_owned())?;
        let mut table = String::new();
        file.take(1_048_576)
            .read_to_string(&mut table)
            .map_err(|_| "Could not read listening sockets".to_owned())?;
        for line in table.lines().skip(1) {
            let fields: Vec<_> = line.split_whitespace().take(10).collect();
            if fields.len() == 10
                && fields[3] == "0A"
                && fields[9]
                    .parse::<u64>()
                    .is_ok_and(|inode| sockets.contains(&inode))
                && let Some(item) = proc_address(fields[1], cfg!(target_endian = "little"))
            {
                listeners.push(item);
            }
        }
    }
    Ok(compact(listeners))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn discover(pid: u32) -> Result<Vec<Listener>, String> {
    run(Command::new("/bin/sh").args(["-c", UNIX_PROBE, "neptune-ports", &pid.to_string()]))
        .map(|text| parse_probe(&text))
}

#[cfg(windows)]
pub fn discover(pid: u32) -> Result<Vec<Listener>, String> {
    let script = format!(
        "$ErrorActionPreference='Stop'; $all=@(Get-CimInstance Win32_Process | Select-Object -First 65536 ProcessId,ParentProcessId); $ids=[System.Collections.Generic.HashSet[uint32]]::new(); [void]$ids.Add({pid}); for($n=0;$n -lt 64;$n++) {{$before=$ids.Count; foreach($p in $all) {{if($ids.Contains($p.ParentProcessId) -and $ids.Count -lt 2048) {{[void]$ids.Add($p.ProcessId)}}}}; if($ids.Count -eq $before) {{break}}}}; Get-NetTCPConnection -State Listen | Where-Object {{$ids.Contains($_.OwningProcess)}} | Select-Object -First 32 | ForEach-Object {{'ip '+$_.LocalAddress+' '+$_.LocalPort}}"
    );
    run(Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]))
        .map(|text| parse_probe(&text))
}

#[cfg(not(any(unix, windows)))]
pub fn discover(_pid: u32) -> Result<Vec<Listener>, String> {
    Err("Port discovery is unavailable on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_urls_use_localhost_for_loopback_and_preserve_the_full_port() {
        for address in ["127.0.0.1", "::1", "0.0.0.0", "::"] {
            for port in [3000, 30000, 65535] {
                let detected = parse_probe(&format!("ip {address} {port}"));
                assert_eq!(detected.len(), 1);
                assert_eq!(detected[0].label(), format!("localhost:{port}"));
                assert_eq!(detected[0].url(), format!("http://localhost:{port}/"));
            }
        }
        let ipv6 = parse_probe("ip ::1 3000")[0];
        assert_eq!(ipv6.authority(), "[::1]:3000");
        let other = parse_probe("ip 192.0.2.1 3000\nip 2001:db8::1 3001");
        assert_eq!(other[0].url(), "http://192.0.2.1:3000/");
        assert_eq!(other[1].url(), "http://[2001:db8::1]:3001/");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ipv6_listener_browser_url_uses_localhost_with_the_detected_port() {
        let socket = std::net::TcpListener::bind("[::1]:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let detected = discover(std::process::id())
            .unwrap()
            .into_iter()
            .find(|item| item.port == port)
            .unwrap();
        assert_eq!(detected.address, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(detected.url(), format!("http://localhost:{port}/"));
    }

    #[test]
    fn probe_decodes_ipv4_ipv6_and_filters_invalid_ports() {
        let result = parse_probe(
            "endian little\ntcp 0100007F:0BB8\ntcp 00000000000000000000000001000000:0BB9\nip 0.0.0.0 3000\nendpoint [::1]:3002\nip 127.0.0.1 0\nip bad 99\n",
        );
        assert_eq!(
            result.iter().map(|p| p.label()).collect::<Vec<_>>(),
            ["localhost:3000", "localhost:3001", "localhost:3002"]
        );
        assert_eq!(result[1].url(), "http://localhost:3001/");
        assert_eq!(
            parse_probe(
                "endian big\ntcp 7F000001:0BB8\ntcp 00000000000000000000000000000001:0BB9\nendpoint [::]:3002\n"
            ),
            result
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn discovery_matches_a_real_listener_and_excludes_unrelated_processes() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        assert!(
            discover(std::process::id())
                .unwrap()
                .iter()
                .any(|item| item.port == port)
        );
        let mut other = Command::new("/bin/sh")
            .args(["-c", "read line"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        assert!(
            !discover(other.id())
                .unwrap()
                .iter()
                .any(|item| item.port == port)
        );
        other.kill().unwrap();
        other.wait().unwrap();
        drop(socket);
        assert!(
            !discover(std::process::id())
                .unwrap()
                .iter()
                .any(|item| item.port == port)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn discovery_follows_nested_shell_children_and_unix_probe_agrees() {
        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready");
        let code = "import socket,sys,time; s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); open(sys.argv[1],'w').write(str(s.getsockname()[1])); time.sleep(15)";
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "sh -c 'python3 -c \"$1\" \"$2\" & wait' nested \"$1\" \"$2\" & wait",
                "pane",
                code,
                ready.to_str().unwrap(),
            ])
            .process_group(0)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let port: u16 = std::fs::read_to_string(ready).unwrap().parse().unwrap();
        let native = discover(child.id()).unwrap();
        let remote = parse_probe(
            &run(Command::new("/bin/sh").args([
                "-c",
                UNIX_PROBE,
                "probe",
                &child.id().to_string(),
            ]))
            .unwrap(),
        );
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .status();
        child.wait().unwrap();
        assert!(native.iter().any(|item| item.port == port));
        assert_eq!(native, remote);
    }
}
