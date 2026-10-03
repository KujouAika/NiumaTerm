use nmt_agent::orchestration::definition::Definition;
use serde_json::json;

use crate::agent_tab::orchestration::properties::ancestors;

#[test]
fn ancestors_are_every_transitive_dependency_in_definition_order() {
    let definition: Definition = serde_json::from_value(json!({
        "version": 1,
        "slots": { "dev": { "profile": { "kind": "claude", "name": "Default" } } },
        "nodes": [
            { "id": "plan", "slot": "dev" },
            { "id": "draft", "slot": "dev", "needs": ["plan"] },
            { "id": "side", "slot": "dev" },
            { "id": "review", "slot": "dev", "needs": ["draft", "missing"] },
        ],
    }))
    .unwrap();

    assert_eq!(ancestors(&definition, 3), ["plan", "draft"]);
    assert!(ancestors(&definition, 0).is_empty());
}

#[test]
fn a_cycle_does_not_make_a_node_its_own_ancestor() {
    let definition: Definition = serde_json::from_value(json!({
        "version": 1,
        "slots": { "dev": { "profile": { "kind": "claude", "name": "Default" } } },
        "nodes": [
            { "id": "a", "slot": "dev", "needs": ["b"] },
            { "id": "b", "slot": "dev", "needs": ["a"] },
        ],
    }))
    .unwrap();

    assert_eq!(ancestors(&definition, 0), ["b"]);
}
