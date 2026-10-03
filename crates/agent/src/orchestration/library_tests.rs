use std::fs;

use tempfile::tempdir;

use crate::orchestration::library::{
    CreateError, create_definition, definitions_directory, load_definitions,
};

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
    assert!(
        entries[0].definition.is_none(),
        "a file that does not decode has no definition"
    );
    assert!(
        entries[1].definition.is_some(),
        "an invalid but decodable file keeps its definition"
    );
    assert!(entries[1].graph.as_ref().unwrap_err()[0].contains("depends on itself"));
    assert!(entries[2].graph.is_ok());
}

#[test]
fn a_missing_directory_lists_nothing() {
    let directory = tempdir().unwrap();

    assert!(load_definitions(directory.path()).unwrap().is_empty());
}

#[test]
fn a_new_definition_holds_only_the_version_and_refuses_a_used_name() {
    let directory = tempdir().unwrap();

    let path = create_definition(directory.path(), "triage").unwrap();

    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{\n  \"version\": 1,\n  \"slots\": {},\n  \"nodes\": []\n}\n"
    );

    let definitions = load_definitions(directory.path()).unwrap();

    assert_eq!(definitions[0].name, "triage");
    assert!(definitions[0].definition.is_some());
    assert!(
        definitions[0].graph.is_err(),
        "a definition without nodes is invalid"
    );

    assert!(matches!(
        create_definition(directory.path(), "triage"),
        Err(CreateError::NameInUse(name)) if name == "triage"
    ));
    assert!(matches!(
        create_definition(directory.path(), "../escape"),
        Err(CreateError::InvalidName)
    ));
    assert!(matches!(
        create_definition(directory.path(), ""),
        Err(CreateError::InvalidName)
    ));
}
