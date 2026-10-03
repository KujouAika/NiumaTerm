use std::fs;

use tempfile::tempdir;

use crate::orchestration::library::{definitions_directory, load_definitions};

const VALID: &str = r#"{
    "version": 1,
    "slots": { "dev": { "profile": { "kind": "claude", "name": "Default" } } },
    "nodes": [{ "id": "plan", "slot": "dev" }]
}"#;

#[test]
fn every_definition_file_is_listed_with_its_errors() {
    let directory = tempdir().unwrap();
    let definitions = definitions_directory(directory.path());

    fs::create_dir_all(&definitions).unwrap();
    fs::write(definitions.join("review.json"), VALID).unwrap();
    fs::write(definitions.join("broken.json"), "{ \"version\": 1,").unwrap();

    fs::write(
        definitions.join("cycle.json"),
        r#"{
            "version": 1,
            "slots": { "dev": { "profile": { "kind": "claude", "name": "Default" } } },
            "nodes": [{ "id": "a", "slot": "dev", "needs": ["a"] }]
        }"#,
    )
    .unwrap();

    fs::write(definitions.join("notes.txt"), "not a definition").unwrap();
    fs::create_dir(definitions.join("nested.json")).unwrap();

    let entries = load_definitions(directory.path()).unwrap();

    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["broken", "cycle", "review"]
    );
    assert!(entries[0].graph.is_err());
    assert!(entries[1].graph.as_ref().unwrap_err()[0].contains("cycle"));
    assert!(entries[2].graph.is_ok());
}

#[test]
fn a_missing_directory_lists_nothing() {
    let directory = tempdir().unwrap();

    assert!(load_definitions(directory.path()).unwrap().is_empty());
}
