use std::fs;

use crate::store::load_or_create_identity;

#[test]
fn the_device_key_is_sealed_on_disk_and_reloads_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let created = load_or_create_identity(dir.path()).unwrap();
    let reloaded = load_or_create_identity(dir.path()).unwrap();

    assert_eq!(created.public(), reloaded.public());
    assert_eq!(created.private(), reloaded.private());

    let sealed = fs::read(dir.path().join("identity.key")).unwrap();

    assert!(
        !sealed.windows(32).any(|window| window == created.private()),
        "the private key is not stored in the clear"
    );
}
