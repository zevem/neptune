use super::*;

#[test]
fn the_peer_check_rejects_a_connection_from_another_user() {
    // No test can connect as another account, so check a real connection's
    // credentials against an owner it does not belong to.
    let (stream, _peer) = UnixStream::pair().unwrap();
    let own = effective_uid();
    check_peer(&stream, own).unwrap();
    let error = check_peer(&stream, own.wrapping_add(1)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
}

/// A connection, once one is free.
fn listening(dir: &Path, version: u32) -> DaemonClient {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match DaemonClient::connect_to(dir, version) {
            Ok(client) => return client,
            Err(error) => {
                assert!(Instant::now() < deadline, "never connected: {error}");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn cat() -> SessionOptions {
    SessionOptions {
        shell: Some("/bin/sh".into()),
        args: vec!["-c".into(), "printf 'UP\\n'; exec cat".into()],
        cols: 40,
        rows: 5,
        scrollback: 16,
        ..SessionOptions::default()
    }
}

/// An update leaves the earlier version's daemon serving its sessions beside
/// the new version's: each keeps its own sessions, and the earlier one exits
/// once its last session is closed while the other keeps running.
#[test]
fn daemons_of_two_versions_serve_one_directory_and_each_exits_once_empty() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("sessions");
    let options = DaemonOptions {
        idle_exit: Duration::from_millis(100),
        ..DaemonOptions::default()
    };
    let (earlier, later) = (PROTOCOL_VERSION, PROTOCOL_VERSION + 1);
    let earlier_daemon = simulated::spawn(&dir, options.clone(), earlier);
    let later_daemon = simulated::spawn(&dir, options, later);

    let old = listening(&dir, earlier);
    let new = listening(&dir, later);
    assert_eq!((old.version(), new.version()), (earlier, later));
    let (kept, kept_id) = old.start(cat(), Arc::new(|| {})).unwrap();
    let (fresh, fresh_id) = new.start(cat(), Arc::new(|| {})).unwrap();
    let ids = |client: &DaemonClient| -> Vec<SessionId> {
        client
            .list()
            .unwrap()
            .into_iter()
            .map(|entry| entry.id)
            .collect()
    };
    assert_eq!(ids(&old), [kept_id]);
    assert_eq!(ids(&new), [fresh_id]);
    // A session of one daemon isn't reachable through the other.
    assert_eq!(
        new.attach(kept_id, 40, 5, Arc::new(|| {}))
            .err()
            .unwrap()
            .kind(),
        SessionErrorKind::NotFound
    );
    // Each version has its own socket and lock.
    for version in [earlier, later] {
        assert!(socket_path(&dir, version).exists());
        assert!(lock_path(&dir, version).exists());
    }

    drop(kept);
    old.close(kept_id).unwrap();
    drop(old);
    earlier_daemon.join().unwrap().unwrap();
    assert!(!socket_path(&dir, earlier).exists());
    // The later daemon still serves its session.
    assert_eq!(ids(&new), [fresh_id]);
    fresh.shutdown();
    drop(fresh);
    drop(new);
    later_daemon.join().unwrap().unwrap();
    assert!(!socket_path(&dir, later).exists());
}

#[test]
fn a_session_id_is_exactly_sixteen_hex_digits() {
    let id: SessionId = "00000000000000a1".parse().unwrap();
    assert_eq!(id.to_string(), "00000000000000a1");
    assert!("00000000000000A1".parse::<SessionId>().is_ok());
    for invalid in [
        "",
        "a1",
        "+0000000000000a1",
        "00000000000000a1 ",
        "0000000000000g01",
    ] {
        assert!(invalid.parse::<SessionId>().is_err(), "{invalid:?}");
    }
}

/// A daemon runs no more sessions than it may, and serves no more
/// connections; it refuses more at once, with why.
#[test]
fn a_daemon_refuses_sessions_and_connections_beyond_its_limits() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("sessions");
    let daemon = simulated::spawn(
        &dir,
        DaemonOptions {
            idle_exit: Duration::from_millis(100),
            max_sessions: 2,
            max_connections: 4,
        },
        PROTOCOL_VERSION,
    );
    let client = listening(&dir, PROTOCOL_VERSION);
    let first = client.start(cat(), Arc::new(|| {})).unwrap();
    let second = client.start(cat(), Arc::new(|| {})).unwrap();
    let refused = client.start(cat(), Arc::new(|| {})).err().unwrap();
    assert_eq!(refused.kind(), SessionErrorKind::Backpressure);
    assert!(refused.to_string().contains("2 sessions"), "{refused}");
    // The client and both sessions' streams: one connection is left.
    let other = listening(&dir, PROTOCOL_VERSION);
    let refused = DaemonClient::connect_to(&dir, PROTOCOL_VERSION)
        .err()
        .unwrap();
    assert_eq!(refused.kind(), SessionErrorKind::Backpressure);
    assert!(
        refused.to_string().contains("too many connections"),
        "{refused}"
    );

    // Ending one makes room for another, and for its connection.
    drop(other);
    first.0.shutdown();
    drop(first);
    let deadline = Instant::now() + Duration::from_secs(5);
    let third = loop {
        match client.start(cat(), Arc::new(|| {})) {
            Ok(started) => break started,
            Err(error) => {
                assert!(Instant::now() < deadline, "never made room: {error}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    client.end_all().unwrap();
    drop((second, third, client));
    daemon.join().unwrap().unwrap();
}

#[test]
fn a_socket_fits_its_address_only_in_a_short_enough_directory() {
    // SAFETY: an all-zero sockaddr_un is valid.
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let limit = address.sun_path.len();
    let at = |length: usize| PathBuf::from(format!("/{}", "d".repeat(length - 1)));
    // `dir/vN` and its terminating NUL.
    let socket = socket_path(Path::new(""), PROTOCOL_VERSION)
        .as_os_str()
        .len()
        + 1;
    assert!(fits(&at(limit - socket - 1)));
    assert!(!fits(&at(limit - socket)));
}
