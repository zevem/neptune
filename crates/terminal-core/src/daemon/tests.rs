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

fn listening(dir: &Path, version: u32) -> DaemonClient {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match DaemonClient::connect_to(dir, version) {
            Ok(client) => return client,
            Err(error) => {
                assert!(Instant::now() < deadline, "never listened: {error}");
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
    };
    let (earlier, later) = (PROTOCOL_VERSION, PROTOCOL_VERSION + 1);
    let serving = (dir.clone(), options.clone());
    let earlier_daemon = std::thread::spawn(move || run(&serving.0, serving.1));
    let serving = (dir.clone(), options);
    let later_daemon = std::thread::spawn(move || simulated::run(&serving.0, serving.1, later));

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
