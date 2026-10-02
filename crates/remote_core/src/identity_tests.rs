use crate::identity::{DeviceId, DeviceKey};

#[test]
fn device_id_is_sixteen_lowercase_symbols_grouped_for_display() {
    let key = DeviceKey::generate().unwrap();
    let id = key.id();

    assert_eq!(id, DeviceId::from_public_key(key.public()));
    assert_eq!(id.as_str().len(), 16);
    assert!(
        id.as_str()
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
    );

    let shown = id.to_string();
    let groups: Vec<&str> = shown.split('-').collect();

    assert_eq!(groups.len(), 4);
    assert_eq!(groups.concat(), id.as_str());
}
