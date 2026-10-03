use std::fs;

use serde::{Deserialize, Serialize};
use tempfile::tempdir;

use crate::snapshot_store::{SnapshotError, SnapshotFormat, SnapshotStore};

const FORMAT: SnapshotFormat = SnapshotFormat {
    file: "item.json",
    field: "item",
    version: 1,
};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Item {
    name: String,
}

fn item(name: &str) -> Item {
    Item { name: name.into() }
}

#[test]
fn saved_files_keep_their_layout_across_open_and_commit() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("item.json");

    fs::write(&path, br#"{"version":1,"revision":4,"item":{"name":"a"}}"#).unwrap();

    let mut store = SnapshotStore::<Item>::open(directory.path().to_path_buf(), &FORMAT).unwrap();

    assert_eq!(store.revision(), 4);
    assert_eq!(store.value(), &item("a"));

    store
        .commit(item("b"), |_, _| Ok::<(), SnapshotError>(()))
        .unwrap();

    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        r#"{"version":1,"revision":5,"item":{"name":"b"}}"#
    );
}

#[test]
fn unknown_field_is_refused_without_touching_the_file() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("item.json");
    let bytes = br#"{"version":1,"revision":0,"item":{"name":"a"},"extra":1}"#;

    fs::write(&path, bytes).unwrap();

    assert!(matches!(
        SnapshotStore::<Item>::open(directory.path().to_path_buf(), &FORMAT),
        Err(SnapshotError::Invalid(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn refused_change_writes_nothing() {
    let directory = tempdir().unwrap();
    let store_directory = directory.path().join("store");

    let mut store = SnapshotStore::create(store_directory.clone(), &FORMAT, item("a")).unwrap();

    let saved = fs::read(store_directory.join("item.json")).unwrap();

    let result = store.commit(item("b"), |previous, next| {
        assert_eq!((previous, next), (&item("a"), &item("b")));

        Err(SnapshotError::Invalid("refused"))
    });

    assert!(matches!(result, Err(SnapshotError::Invalid("refused"))));
    assert_eq!(store.revision(), 0);
    assert_eq!(store.value(), &item("a"));
    assert_eq!(fs::read(store_directory.join("item.json")).unwrap(), saved);
}
