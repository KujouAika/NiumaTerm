use serde_json::{Value, json};

use crate::orchestration::definition::Definition;
use crate::orchestration::graph::{DefinitionError, Graph};
use crate::orchestration::template::TemplateError;

fn slot() -> Value {
    json!({ "profile": { "kind": "claude", "name": "Default" } })
}

fn definition(slots: &[&str], nodes: Value) -> Definition {
    let slots: serde_json::Map<String, Value> = slots
        .iter()
        .map(|name| ((*name).to_owned(), slot()))
        .collect();

    serde_json::from_value(json!({ "version": 1, "slots": slots, "nodes": nodes })).unwrap()
}

fn errors(definition: Definition) -> Vec<DefinitionError> {
    Graph::new(definition).unwrap_err()
}

#[test]
fn two_step_definition_runs_its_steps_in_order() {
    let graph = Graph::new(definition(
        &["dev"],
        json!([
            { "id": "plan", "slot": "dev", "prompt": "Plan {{input}}" },
            { "id": "implement", "slot": "dev", "needs": ["plan"] },
        ]),
    ))
    .unwrap();

    assert_eq!(graph.order(), [0, 1]);
    assert_eq!(graph.needs(1), [0]);
    assert!(graph.is_ancestor(0, 1));
    assert!(!graph.is_ancestor(1, 0));
    assert_eq!((graph.depth(0), graph.depth(1)), (0, 1));
    assert_eq!(graph.max_parallel(), 3);
}

#[test]
fn order_puts_dependencies_first_even_when_listed_later() {
    let graph = Graph::new(definition(
        &["a", "b"],
        json!([
            { "id": "review", "slot": "b", "needs": ["implement"] },
            { "id": "implement", "slot": "a", "needs": ["plan"] },
            { "id": "plan", "slot": "a" },
        ]),
    ))
    .unwrap();

    assert_eq!(graph.order(), [2, 1, 0]);
    assert!(graph.is_ancestor(2, 0));
    assert_eq!(graph.depth(0), 2);
}

#[test]
fn cycles_are_rejected_naming_their_nodes() {
    assert_eq!(
        errors(definition(
            &["dev", "critic"],
            json!([
                { "id": "a", "slot": "dev", "needs": ["b"] },
                { "id": "b", "slot": "critic", "needs": ["a"] },
            ]),
        )),
        [DefinitionError::Cycle(vec!["a".into(), "b".into()])]
    );

    assert_eq!(
        errors(definition(
            &["dev"],
            json!([{ "id": "a", "slot": "dev", "needs": ["a"] }]),
        )),
        [DefinitionError::Cycle(vec!["a".into()])]
    );

    assert_eq!(
        DefinitionError::Cycle(vec!["a".into()]).to_string(),
        "node `a` depends on itself"
    );
    assert_eq!(
        DefinitionError::Cycle(vec!["a".into(), "b".into()]).to_string(),
        "nodes `a`, `b` depend on each other in a cycle"
    );
}

#[test]
fn templates_may_read_only_ancestors() {
    assert_eq!(
        errors(definition(
            &["dev", "critic", "tester"],
            json!([
                { "id": "plan", "slot": "dev" },
                { "id": "test", "slot": "tester", "needs": ["plan"] },
                { "id": "review", "slot": "critic", "needs": ["plan"],
                  "prompt": "{{plan.output}} {{test.output}} {{missing.output}}" },
            ]),
        )),
        [
            DefinitionError::NotAncestor {
                node: "review".into(),
                reference: "test".into(),
            },
            DefinitionError::NotAncestor {
                node: "review".into(),
                reference: "missing".into(),
            },
        ]
    );
}

#[test]
fn nodes_sharing_a_slot_must_be_ordered() {
    assert_eq!(
        errors(definition(
            &["dev"],
            json!([
                { "id": "plan", "slot": "dev" },
                { "id": "frontend", "slot": "dev", "needs": ["plan"] },
                { "id": "backend", "slot": "dev", "needs": ["plan"] },
            ]),
        )),
        [DefinitionError::UnorderedSlot {
            slot: "dev".into(),
            first: "frontend".into(),
            second: "backend".into(),
        }]
    );

    // `plan` and `test` share a slot through a node on another slot.
    assert!(
        Graph::new(definition(
            &["dev", "critic"],
            json!([
                { "id": "plan", "slot": "dev" },
                { "id": "review", "slot": "critic", "needs": ["plan"] },
                { "id": "test", "slot": "dev", "needs": ["review"] },
            ]),
        ))
        .is_ok()
    );
}

#[test]
fn structural_errors_are_reported_together() {
    let mut broken = definition(
        &["dev"],
        json!([
            { "id": "Plan", "slot": "dev" },
            { "id": "build", "slot": "ops", "needs": ["deploy", "Plan", "Plan"] },
            { "id": "build", "slot": "dev", "prompt": "  " },
            { "id": "check", "slot": "dev", "prompt": "{{plan}}" },
        ]),
    );

    broken.version = 2;
    broken.max_parallel = Some(9);

    assert_eq!(
        errors(broken),
        [
            DefinitionError::UnsupportedVersion(2),
            DefinitionError::MaxParallelOutOfRange(9),
            DefinitionError::InvalidNodeId("Plan".into()),
            DefinitionError::DuplicateNode("build".into()),
            DefinitionError::UnknownSlot {
                node: "build".into(),
                slot: "ops".into(),
            },
            DefinitionError::UnknownDependency {
                node: "build".into(),
                dependency: "deploy".into(),
            },
            DefinitionError::RepeatedDependency {
                node: "build".into(),
                dependency: "Plan".into(),
            },
            DefinitionError::EmptyPrompt {
                node: "build".into(),
            },
            DefinitionError::Template {
                node: "check".into(),
                error: TemplateError::UnknownReference("plan".into()),
            },
        ]
    );
}

#[test]
fn duplicate_slot_names_in_the_file_are_not_merged() {
    let source = r#"{
        "version": 1,
        "slots": {
            "dev": { "profile": { "kind": "claude", "name": "Default" } },
            "dev": { "profile": { "kind": "codex", "name": "Default" } }
        },
        "nodes": [{ "id": "plan", "slot": "dev" }]
    }"#;

    let definition: Definition = serde_json::from_str(source).unwrap();

    assert_eq!(definition.slots.len(), 2);
    assert_eq!(
        errors(definition),
        [DefinitionError::DuplicateSlot("dev".into())]
    );
}

#[test]
fn graph_size_limits() {
    assert_eq!(
        errors(definition(&["dev"], json!([]))),
        [DefinitionError::NoNodes]
    );

    let chain: Vec<Value> = (0..33)
        .map(|index| match index {
            0 => json!({ "id": "n0", "slot": "dev" }),
            _ => json!({ "id": format!("n{index}"), "slot": "dev",
                         "needs": [format!("n{}", index - 1)] }),
        })
        .collect();

    assert_eq!(
        errors(definition(&["dev"], Value::Array(chain[..33].to_vec()))),
        [DefinitionError::TooManyNodes(33)]
    );
    assert!(Graph::new(definition(&["dev"], Value::Array(chain[..32].to_vec()))).is_ok());
}

#[test]
fn credentials_and_unknown_fields_are_refused_by_name() {
    let mut leaking = slot();

    leaking["api_key"] = json!("sk-secret");

    let error = serde_json::from_value::<Definition>(json!({
        "version": 1,
        "slots": { "dev": leaking },
        "nodes": [{ "id": "plan", "slot": "dev" }],
    }))
    .unwrap_err();

    assert!(error.to_string().contains("api_key"), "{error}");

    let error = serde_json::from_value::<Definition>(json!({
        "version": 1,
        "slots": { "dev": slot() },
        "nodes": [{ "id": "plan", "slot": "dev", "need": ["x"] }],
    }))
    .unwrap_err();

    assert!(error.to_string().contains("need"), "{error}");
}

#[test]
fn run_input_is_needed_only_when_some_prompt_contains_it() {
    let graph = |nodes| Graph::new(definition(&["dev"], nodes)).unwrap();

    assert!(graph(json!([{ "id": "plan", "slot": "dev" }])).uses_input());
    assert!(graph(json!([{ "id": "plan", "slot": "dev", "prompt": "Do {{input}}" }])).uses_input());
    assert!(
        !graph(json!([
            { "id": "plan", "slot": "dev", "prompt": "Summarize the repository." },
            { "id": "review", "slot": "dev", "needs": ["plan"] },
        ]))
        .uses_input()
    );
}

#[test]
fn a_slot_needs_a_name() {
    assert_eq!(
        errors(definition(&[""], json!([{ "id": "plan", "slot": "" }]))),
        [DefinitionError::EmptySlotName]
    );
}

#[test]
fn layout_is_optional_and_ignored_by_validation() {
    let source = json!({
        "version": 1,
        "slots": { "dev": slot() },
        "nodes": [{ "id": "plan", "slot": "dev" }],
        "layout": {
            "plan": { "x": 40.0, "y": 80.0 },
            "draft": { "x": 0.0, "y": 0.0 },
        },
    });

    let definition: Definition = serde_json::from_value(source).unwrap();

    assert_eq!(definition.layout.len(), 2);
    assert!(Graph::new(definition).is_ok());

    let without: Definition = serde_json::from_value(json!({
        "version": 1,
        "slots": { "dev": slot() },
        "nodes": [{ "id": "plan", "slot": "dev" }],
    }))
    .unwrap();

    assert!(without.layout.is_empty());
    assert!(!serde_json::to_string(&without).unwrap().contains("layout"));
}

#[test]
fn errors_name_the_nodes_and_slots_they_are_about() {
    let cycle = DefinitionError::Cycle(vec!["plan".into(), "review".into()]);

    assert_eq!(cycle.nodes(), ["plan", "review"]);
    assert!(cycle.slots().is_empty());

    let unordered = DefinitionError::UnorderedSlot {
        slot: "dev".into(),
        first: "a".into(),
        second: "b".into(),
    };

    assert_eq!(unordered.nodes(), ["a", "b"]);
    assert_eq!(unordered.slots(), ["dev"]);

    let unknown = DefinitionError::UnknownSlot {
        node: "a".into(),
        slot: "web".into(),
    };

    assert_eq!(unknown.nodes(), ["a"]);
    assert!(unknown.slots().is_empty());
    assert!(DefinitionError::NoNodes.nodes().is_empty());
}
