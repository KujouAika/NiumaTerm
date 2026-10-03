use serde_json::json;

use crate::orchestration::compose::compose;
use crate::orchestration::definition::Definition;
use crate::orchestration::graph::Graph;

fn graph() -> Graph {
    let definition: Definition = serde_json::from_value(json!({
        "version": 1,
        "slots": {
            "dev": { "profile": { "kind": "claude", "name": "Default" } },
            "critic": { "profile": { "kind": "codex", "name": "Default" } },
        },
        "nodes": [
            { "id": "plan", "slot": "dev" },
            { "id": "frontend", "slot": "dev", "needs": ["plan"] },
            { "id": "backend", "slot": "critic", "needs": ["plan"] },
            { "id": "merge", "slot": "dev", "needs": ["backend", "frontend"] },
            { "id": "review", "slot": "critic", "needs": ["merge"],
              "prompt": "Check this against the plan:\n{{plan.output}}\nTask: {{input}}" },
        ],
    }))
    .unwrap();

    Graph::new(definition).unwrap()
}

fn outputs() -> Vec<Option<String>> {
    ["PLAN", "FRONT", "BACK", "MERGED"]
        .into_iter()
        .map(|output| Some(output.to_owned()))
        .chain([None])
        .collect()
}

#[test]
fn a_root_without_template_sends_the_input() {
    assert_eq!(
        compose(&graph(), 0, "Add search", None, &outputs()).as_deref(),
        Some("Add search")
    );
}

#[test]
fn a_template_reads_the_input_and_ancestor_outputs() {
    assert_eq!(
        compose(&graph(), 4, "Add search", None, &outputs()).as_deref(),
        Some("Check this against the plan:\nPLAN\nTask: Add search")
    );
}

#[test]
fn dependencies_are_headed_in_the_order_the_node_lists_them() {
    assert_eq!(
        compose(&graph(), 3, "Add search", None, &outputs()).as_deref(),
        Some("## backend\n\nBACK\n\n## frontend\n\nFRONT")
    );
}

#[test]
fn the_role_opens_only_the_text_it_is_given_for() {
    let graph = graph();

    assert_eq!(
        compose(&graph, 0, "Add search", Some("You implement."), &outputs()).as_deref(),
        Some("You implement.\n\nAdd search")
    );
    assert_eq!(
        compose(&graph, 0, "Add search", Some(""), &outputs()).as_deref(),
        Some("Add search")
    );
}

#[test]
fn a_missing_output_composes_nothing() {
    let mut outputs = outputs();

    outputs[0] = None;

    assert_eq!(compose(&graph(), 4, "Add search", None, &outputs), None);
    assert_eq!(compose(&graph(), 1, "Add search", None, &outputs), None);
}
