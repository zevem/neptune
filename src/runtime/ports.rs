//! One coalescing worker owns discovery and SSH forwards for all pane generations.
use crate::platform::ports::{self, Listener};
use neptune_model::PaneId;
use std::{
    collections::{BTreeMap, VecDeque},
    net::{Ipv4Addr, TcpListener},
    process::{Child, Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

const MAX_FORWARDS: usize = 64;

#[derive(Clone)]
pub struct RemoteTarget {
    pub client: String,
    pub destination: String,
    // Keeps the private directory alive until in-flight helpers finish.
    pub control: Option<Arc<tempfile::TempDir>>,
}
impl RemoteTarget {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.client);
        command.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=2"]);
        if let Some(control) = &self.control {
            command.arg("-S").arg(control.path().join("control"));
            // A missing master must fail, never silently open another session.
            command.args(["-o", "ProxyCommand=false"]);
        }
        command
    }
    fn control_command(&self) -> Result<Command, String> {
        let control = self
            .control
            .as_ref()
            .ok_or("SSH connection sharing is unavailable")?;
        let mut command = Command::new(&self.client);
        // Mux operations need only the exact private socket. Skip configured
        // forwards so a request/cancel cannot affect unrelated user forwards.
        command
            .args(["-F", "none", "-S"])
            .arg(control.path().join("control"));
        Ok(command)
    }
    fn check(&self) -> Result<(), String> {
        if self.control.is_some() {
            ports::run(
                self.control_command()?
                    .args(["-O", "check", "--", &self.destination]),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Target {
    pub pane: PaneId,
    pub generation: u64,
    pub pid: u32,
    pub remote: Option<RemoteTarget>,
}
impl Target {
    fn same(&self, other: &Self) -> bool {
        self.pane == other.pane
            && self.generation == other.generation
            && self.pid == other.pid
            && self
                .remote
                .as_ref()
                .map(|r| (&r.destination, r.control.as_ref().map(|c| c.path())))
                == other
                    .remote
                    .as_ref()
                    .map(|r| (&r.destination, r.control.as_ref().map(|c| c.path())))
    }
    fn discover(&self) -> Result<Vec<Listener>, String> {
        match &self.remote {
            None => ports::discover(self.pid),
            Some(remote) => {
                remote.check()?;
                let script = ports::UNIX_PROBE.replace('\'', "'\\''");
                let program = format!("sh -c '{script}' neptune-ports {}", self.pid);
                ports::run(remote.command().args([
                    "-o",
                    "ClearAllForwardings=yes",
                    "-T",
                    "--",
                    &remote.destination,
                    &program,
                ]))
                .map(|text| ports::parse_probe(&text))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Forward {
    Off,
    Starting,
    On(u16),
    Stopping,
    Failed { message: String, local: Option<u16> },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub listener: Listener,
    pub remote: bool,
    pub forward: Forward,
}
impl Port {
    pub fn label(&self) -> String {
        match self.forward {
            Forward::On(local)
            | Forward::Failed {
                local: Some(local), ..
            } => format!("localhost:{local}"),
            _ => self.listener.label(),
        }
    }
    pub fn url(&self) -> Option<String> {
        match self.forward {
            Forward::On(local)
            | Forward::Failed {
                local: Some(local), ..
            } => Some(format!("http://localhost:{local}/")),
            _ if !self.remote => Some(self.listener.url()),
            _ => None,
        }
    }
}

struct Request {
    pane: PaneId,
    generation: u64,
    listener: Listener,
    stop: bool,
}
#[derive(Default)]
struct Shared {
    targets: BTreeMap<PaneId, Target>,
    requests: VecDeque<Request>,
    updates: BTreeMap<PaneId, (u64, Vec<Port>)>,
    stop: bool,
}
#[derive(Default)]
pub struct Ports {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
    targets: BTreeMap<PaneId, Target>,
    views: BTreeMap<PaneId, (u64, Vec<Port>)>,
}
impl Ports {
    pub fn sync(&mut self, targets: Vec<Target>, wake: Arc<dyn Fn() + Send + Sync>) {
        let targets: BTreeMap<_, _> = targets.into_iter().take(64).map(|t| (t.pane, t)).collect();
        self.views.retain(|pane, (generation, _)| {
            targets
                .get(pane)
                .is_some_and(|t| t.generation == *generation)
        });
        let changed = targets.len() != self.targets.len()
            || targets
                .iter()
                .any(|(pane, target)| self.targets.get(pane).is_none_or(|old| !target.same(old)));
        if changed {
            if let Ok(mut shared) = self.shared.0.lock() {
                shared.targets = targets.clone();
                shared.requests.retain(|r| {
                    targets
                        .get(&r.pane)
                        .is_some_and(|t| t.generation == r.generation)
                });
                self.shared.1.notify_one();
            }
            self.targets = targets;
        }
        if self.worker.is_none() && !self.targets.is_empty() {
            let shared = self.shared.clone();
            self.worker = std::thread::Builder::new()
                .name("neptune-ports".into())
                .spawn(move || worker(shared, wake))
                .ok();
        }
        if let Ok(mut shared) = self.shared.0.lock() {
            for (pane, (generation, ports)) in std::mem::take(&mut shared.updates) {
                if self
                    .targets
                    .get(&pane)
                    .is_some_and(|t| t.generation == generation)
                {
                    self.views.insert(pane, (generation, ports));
                }
            }
        }
    }
    pub fn view(&self, pane: PaneId, generation: u64) -> &[Port] {
        self.views
            .get(&pane)
            .filter(|(g, _)| *g == generation)
            .map_or(&[], |(_, ports)| ports)
    }
    /// Intentional exit only; interactive close/restart never joins a worker.
    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        if let Ok(mut shared) = self.shared.0.lock() {
            shared.stop = true;
            self.shared.1.notify_one();
        }
        let deadline = Instant::now() + timeout;
        while self.worker.as_ref().is_some_and(|w| !w.is_finished()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.worker.as_ref().is_some_and(|w| w.is_finished())
            && let Some(worker) = self.worker.take()
        {
            return worker.join().is_ok();
        }
        self.worker.is_none()
    }
    pub fn request(
        &mut self,
        pane: PaneId,
        generation: u64,
        listener: Listener,
        stop: bool,
    ) -> Result<(), &'static str> {
        let port = self
            .view(pane, generation)
            .iter()
            .find(|p| p.listener == listener)
            .ok_or("This server is no longer running")?;
        if !port.remote || matches!(port.forward, Forward::Starting | Forward::Stopping) {
            return Ok(());
        }
        let mut shared = self.shared.0.lock().map_err(|_| "Port worker stopped")?;
        if shared.requests.len() >= 32 {
            return Err("Too many forwarding requests; try again shortly");
        }
        shared.requests.push_back(Request {
            pane,
            generation,
            listener,
            stop,
        });
        shared.updates.remove(&pane);
        if let Some((_, ports)) = self.views.get_mut(&pane)
            && let Some(port) = ports.iter_mut().find(|p| p.listener == listener)
        {
            port.forward = if stop {
                Forward::Stopping
            } else {
                Forward::Starting
            };
        }
        self.shared.1.notify_one();
        Ok(())
    }
}
impl Drop for Ports {
    fn drop(&mut self) {
        if let Ok(mut shared) = self.shared.0.lock() {
            shared.stop = true;
            self.shared.1.notify_one();
        }
    }
}

struct State {
    target: Target,
    ports: Vec<Port>,
    next_scan: Instant,
    tunnels: BTreeMap<Listener, Tunnel>,
}

/// Separate clients (Windows OpenSSH has no multiplexing) are owned/reaped on
/// the port worker, including disappearance, restart and application shutdown.
struct Tunnel(Child);
impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn separate_tunnel(
    remote: &RemoteTarget,
    listener: Listener,
    local: u16,
) -> Result<Tunnel, String> {
    let mut command = remote.command();
    command.args([
        "-N",
        "-T",
        "-o",
        "ExitOnForwardFailure=yes",
        "-L",
        &format!("127.0.0.1:{local}:{}", listener.authority()),
        "--",
        &remote.destination,
    ]);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut tunnel = Tunnel(
        command
            .spawn()
            .map_err(|_| "Could not start SSH forwarding".to_owned())?,
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match tunnel.0.try_wait() {
            Ok(Some(_)) | Err(_) => {
                return Err(
                    "SSH forwarding failed. Check key or agent authentication and the local port."
                        .into(),
                );
            }
            Ok(None) => {}
        }
        if std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, local)),
            Duration::from_millis(50),
        )
        .is_ok()
        {
            return Ok(tunnel);
        }
        if Instant::now() >= deadline {
            return Err("SSH forwarding timed out. Check key or agent authentication.".into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn forwarding(
    remote: &RemoteTarget,
    listener: Listener,
    local: u16,
    stop: bool,
) -> Result<(), String> {
    if remote.control.is_none() {
        return Err("This SSH client cannot share its connection for forwarding".into());
    }
    remote.check()?;
    let spec = format!("127.0.0.1:{local}:{}", listener.authority());
    ports::run(remote.control_command()?.args([
        "-o",
        "ExitOnForwardFailure=yes",
        "-O",
        if stop { "cancel" } else { "forward" },
        "-L",
        &spec,
        "--",
        &remote.destination,
    ]))?;
    Ok(())
}

fn publish(shared: &Arc<(Mutex<Shared>, Condvar)>, state: &State, wake: &dyn Fn()) {
    if let Ok(mut shared) = shared.0.lock()
        && shared
            .targets
            .get(&state.target.pane)
            .is_some_and(|t| t.same(&state.target))
    {
        shared.updates.insert(
            state.target.pane,
            (state.target.generation, state.ports.clone()),
        );
        drop(shared);
        wake();
    }
}

fn worker(shared: Arc<(Mutex<Shared>, Condvar)>, wake: Arc<dyn Fn() + Send + Sync>) {
    let mut states: BTreeMap<PaneId, State> = BTreeMap::new();
    loop {
        let (targets, requests) = {
            let Ok(mut mailbox) = shared.0.lock() else {
                return;
            };
            if mailbox.stop {
                return;
            } // Pane-owned SSH masters die with their PTYs.
            (
                mailbox.targets.clone(),
                std::mem::take(&mut mailbox.requests),
            )
        };
        states.retain(|pane, state| targets.get(pane).is_some_and(|t| t.same(&state.target)));
        for (pane, target) in targets {
            states.entry(pane).or_insert_with(|| State {
                target,
                ports: Vec::new(),
                next_scan: Instant::now(),
                tunnels: BTreeMap::new(),
            });
        }
        for request in requests {
            if shared.0.lock().map_or(true, |mailbox| mailbox.stop) {
                return;
            }
            let at_capacity = states
                .values()
                .flat_map(|state| &state.ports)
                .filter(|port| {
                    matches!(
                        port.forward,
                        Forward::On(_) | Forward::Failed { local: Some(_), .. }
                    )
                })
                .count()
                >= MAX_FORWARDS;
            let Some(state) = states
                .get_mut(&request.pane)
                .filter(|s| s.target.generation == request.generation)
            else {
                continue;
            };
            if !state.ports.iter().any(|p| p.listener == request.listener) {
                publish(&shared, state, &*wake);
                continue;
            }
            let Some(port) = state
                .ports
                .iter_mut()
                .find(|p| p.listener == request.listener)
            else {
                continue;
            };
            let Some(remote) = &state.target.remote else {
                continue;
            };
            if request.stop {
                if let Forward::On(local)
                | Forward::Failed {
                    local: Some(local), ..
                } = port.forward
                {
                    let result = if state.tunnels.remove(&port.listener).is_some() {
                        Ok(())
                    } else {
                        forwarding(remote, port.listener, local, true)
                    };
                    port.forward = match result {
                        Ok(()) => Forward::Off,
                        Err(message) => Forward::Failed {
                            message,
                            local: Some(local),
                        },
                    };
                }
            } else if port.url().is_none() {
                if at_capacity {
                    port.forward = Forward::Failed {
                        message:
                            "Up to 64 ports may be forwarded. Stop another forward before retrying."
                                .into(),
                        local: None,
                    };
                    publish(&shared, state, &*wake);
                    continue;
                }
                let local = TcpListener::bind((Ipv4Addr::LOCALHOST, port.listener.port))
                    .or_else(|_| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))
                    .and_then(|socket| socket.local_addr().map(|a| a.port()));
                port.forward = match local
                    .map_err(|_| "No local forwarding port is available".to_owned())
                    .and_then(|local| {
                        if remote.control.is_some() {
                            forwarding(remote, port.listener, local, false)?;
                        } else {
                            state.tunnels.insert(
                                port.listener,
                                separate_tunnel(remote, port.listener, local)?,
                            );
                        }
                        Ok(local)
                    }) {
                    Ok(local) => Forward::On(local),
                    Err(message) => Forward::Failed {
                        message,
                        local: None,
                    },
                };
            }
            publish(&shared, state, &*wake);
        }
        // One scan at a time; forward/cancel requests run between scans. The
        // oldest deadline goes first, so hidden panes cannot starve.
        if let Some((_, state)) = states
            .iter_mut()
            .filter(|(_, s)| s.next_scan <= Instant::now())
            .min_by_key(|(_, s)| s.next_scan)
        {
            state.next_scan = Instant::now() + Duration::from_secs(2);
            let Ok(listeners) = state.target.discover() else {
                continue;
            };
            let before = state.ports.clone();
            let mut previous = std::mem::take(&mut state.ports);
            state.ports = listeners
                .into_iter()
                .map(|listener| {
                    if let Some(index) = previous.iter().position(|p| p.listener == listener) {
                        previous.remove(index)
                    } else {
                        Port {
                            listener,
                            remote: state.target.remote.is_some(),
                            forward: Forward::Off,
                        }
                    }
                })
                .collect();
            for mut port in previous {
                if state.tunnels.remove(&port.listener).is_none()
                    && let (
                        Some(remote),
                        Forward::On(local)
                        | Forward::Failed {
                            local: Some(local), ..
                        },
                    ) = (&state.target.remote, &port.forward)
                {
                    // A failed cancellation still owns a local listener.
                    // Retain its chip for Stop/retry and retry on the next scan.
                    if let Err(message) = forwarding(remote, port.listener, *local, true) {
                        port.forward = Forward::Failed {
                            message,
                            local: Some(*local),
                        };
                        state.ports.push(port);
                    }
                }
            }
            // Preserve active forwards at the pane's port limit before adding
            // newly discovered servers; failed cleanup cannot grow state.
            state.ports.sort_by_key(|port| {
                (
                    !matches!(
                        port.forward,
                        Forward::On(_) | Forward::Failed { local: Some(_), .. }
                    ),
                    port.listener,
                )
            });
            state.ports.truncate(ports::MAX_PORTS);
            for (listener, tunnel) in &mut state.tunnels {
                if !matches!(tunnel.0.try_wait(), Ok(None))
                    && let Some(port) = state.ports.iter_mut().find(|p| p.listener == *listener)
                {
                    port.forward = Forward::Failed {
                        message: "SSH forwarding disconnected; click to retry".into(),
                        local: None,
                    };
                }
            }
            state
                .tunnels
                .retain(|_, tunnel| matches!(tunnel.0.try_wait(), Ok(None)));
            state.next_scan = Instant::now() + Duration::from_secs(2);
            // Coalesced results wake only when what the user can see changes.
            if before != state.ports {
                publish(&shared, state, &*wake);
            }
            continue;
        }
        let wait = states
            .values()
            .map(|s| s.next_scan.saturating_duration_since(Instant::now()))
            .min()
            .unwrap_or(Duration::from_secs(60));
        let Ok(mailbox) = shared.0.lock() else {
            return;
        };
        if mailbox.stop {
            return;
        }
        if mailbox.requests.is_empty() {
            let _ = shared
                .1
                .wait_timeout(mailbox, wait.min(Duration::from_secs(2)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_discovers_hidden_targets_sleeps_without_repaint_and_rejects_old_generations() {
        use std::io::BufRead;
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Fixture(Child);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut fixture = Fixture(Command::new("python3")
            .args(["-c", "import socket,sys; s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); print(s.getsockname()[1],flush=True); sys.stdin.read()"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap());
        let pid = fixture.0.id();
        let mut ready = String::new();
        std::io::BufReader::new(fixture.0.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        let listener = Listener {
            address: Ipv4Addr::LOCALHOST.into(),
            port: ready.trim().parse().unwrap(),
        };
        let pane = PaneId::new(17);
        let mut monitor = Ports::default();
        let wakes = Arc::new(AtomicUsize::new(0));
        let count = wakes.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let targets = || {
            vec![Target {
                pane,
                generation: 1,
                pid,
                remote: None,
            }]
        };
        monitor.sync(targets(), wake.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !monitor.view(pane, 1).iter().any(|p| p.listener == listener) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
            monitor.sync(targets(), wake.clone());
        }
        let before = wakes.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(2200));
        monitor.sync(targets(), wake.clone());
        assert_eq!(wakes.load(Ordering::SeqCst), before);
        assert!(monitor.view(pane, 2).is_empty());
        assert!(monitor.request(pane, 2, listener, false).is_err());
        let stale = Port {
            listener: Listener {
                port: 1,
                ..listener
            },
            remote: false,
            forward: Forward::Off,
        };
        monitor
            .shared
            .0
            .lock()
            .unwrap()
            .updates
            .insert(pane, (1, vec![stale]));
        monitor.sync(
            vec![Target {
                pane,
                generation: 2,
                pid,
                remote: None,
            }],
            wake.clone(),
        );
        assert!(monitor.view(pane, 1).is_empty());
        assert!(
            !monitor
                .view(pane, 2)
                .iter()
                .any(|port| port.listener.port == 1)
        );
        monitor.sync(Vec::new(), wake);
        assert!(monitor.views.is_empty());
        assert!(monitor.shutdown(Duration::from_secs(4)));
    }

    #[test]
    fn forwarded_urls_show_the_local_port_and_cancellation_failure_retains_the_forward() {
        let mut port = Port {
            listener: Listener {
                address: Ipv4Addr::LOCALHOST.into(),
                port: 3000,
            },
            remote: true,
            forward: Forward::Off,
        };
        assert!(port.url().is_none());
        port.forward = Forward::On(4000);
        assert_eq!(port.label(), "localhost:4000");
        assert_eq!(port.url().as_deref(), Some("http://localhost:4000/"));
        port.forward = Forward::Failed {
            message: "Could not cancel".into(),
            local: Some(4000),
        };
        assert_eq!(port.url().as_deref(), Some("http://localhost:4000/"));
        port.forward = Forward::Off;
        assert!(port.url().is_none());
    }
}
