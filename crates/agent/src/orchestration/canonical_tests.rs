use crate::orchestration::canonical::to_canonical_json;
use crate::orchestration::definition::Definition;

/// A hand-written file: compact nodes, keys out of order, a slot setting
/// left null, a layout entry for a removed node, and a fractional position.
const HAND_WRITTEN: &str = r#"{"slots": {"dev": {"settings": {"model": "opus", "effort": null}, "profile": {"name": "Default", "kind": "claude"}, "role": "You plan."}, "web": {"profile": {"kind": "codex", "name": "Web"}}},
  "version": 1,
  "nodes": [ {"slot": "dev", "id": "plan", "prompt": "Plan {{input}}"}, {"needs": ["plan"], "id": "build", "slot": "web"} ],
  "layout": {"draft": {"x": 1, "y": 2}, "build": {"y": 40.4, "x": 300.6}, "plan": {"x": 0, "y": 0}}}"#;

const CANONICAL: &str = r#"{
  "version": 1,
  "slots": {
    "dev": {
      "profile": {
        "kind": "claude",
        "name": "Default"
      },
      "role": "You plan.",
      "settings": {
        "model": "opus"
      }
    },
    "web": {
      "profile": {
        "kind": "codex",
        "name": "Web"
      }
    }
  },
  "nodes": [
    {
      "id": "plan",
      "slot": "dev",
      "prompt": "Plan {{input}}"
    },
    {
      "id": "build",
      "slot": "web",
      "needs": [
        "plan"
      ]
    }
  ],
  "layout": {
    "plan": {
      "x": 0,
      "y": 0
    },
    "build": {
      "x": 301,
      "y": 40
    }
  }
}
"#;

fn decode(text: &str) -> Definition {
    serde_json::from_str(text).unwrap()
}

#[test]
fn a_definition_is_written_in_the_canonical_format() {
    assert_eq!(to_canonical_json(&decode(HAND_WRITTEN)), CANONICAL);
}

#[test]
fn writing_again_changes_nothing() {
    let once = to_canonical_json(&decode(HAND_WRITTEN));

    assert_eq!(to_canonical_json(&decode(&once)), once);
}

#[test]
fn a_hand_written_file_keeps_its_meaning() {
    let original = decode(HAND_WRITTEN);
    let written = decode(&to_canonical_json(&original));

    assert_eq!(written.version, original.version);
    assert_eq!(written.max_parallel, original.max_parallel);
    assert_eq!(written.slots, original.slots);
    assert_eq!(written.nodes, original.nodes);
}

#[test]
fn optional_parts_appear_only_when_set() {
    let mut definition = decode(r#"{"version": 1, "slots": {}, "nodes": []}"#);

    assert_eq!(
        to_canonical_json(&definition),
        "{\n  \"version\": 1,\n  \"slots\": {},\n  \"nodes\": []\n}\n"
    );

    definition.max_parallel = Some(2);

    assert!(to_canonical_json(&definition).contains("\"max_parallel\": 2,\n  \"slots\""));
}
