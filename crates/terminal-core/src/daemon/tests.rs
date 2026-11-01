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
