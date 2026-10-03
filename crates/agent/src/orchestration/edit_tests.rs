use serde_json::json;

use crate::agent_spec::ProfileReference;
use crate::chat::ThreadSettings;
use crate::orchestration::definition::{Definition, Node, Position, SlotBody};
use crate::orchestration::edit::{Edit, Editor};
use crate::orchestration::graph::DefinitionError;
use crate::session::AgentKind;

/// `plan` on `dev`, `review` on `critic` reading `plan`'s output.
fn editor() -> Editor {
    let definition: Definition = serde_json::from_value(json!({
        "version": 1,
        "slots": {
            "dev": { "profile": { "kind": "claude", "name": "Default" } },
            "critic": { "profile": { "kind": "codex", "name": "Review" } },
        },
        "nodes": [
            { "id": "plan", "slot": "dev" },
            { "id": "review", "slot": "critic", "needs": ["plan"],
              "prompt": "Check {{plan.output}}, not \\{{plan.output}}" },
        ],
        "layout": { "plan": { "x": 10.0, "y": 20.0 } },
    }))
    .unwrap();

    Editor::open(definition)
}

fn at(x: f32, y: f32) -> Position {
    Position { x, y }
}

fn node<'a>(editor: &'a Editor, id: &str) -> &'a Node {
    editor
        .definition()
        .nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap()
}

#[test]
fn moving_rounds_and_touches_only_the_layout() {
    let mut editor = editor();

    let nodes = editor.definition().nodes.clone();

    assert!(editor.apply(Edit::MoveNodes(vec![
        ("plan".into(), at(100.4, 200.6)),
        ("ghost".into(), at(1., 1.)),
    ])));

    assert_eq!(editor.definition().layout["plan"], at(100., 201.));
    assert!(!editor.definition().layout.contains_key("ghost"));
    assert_eq!(editor.definition().nodes, nodes);
}

#[test]
fn a_new_node_gets_an_unused_id_and_the_first_slot() {
    let mut editor = editor();

    let edit = editor.new_node(at(0., 0.));

    assert_eq!(
        edit,
        Edit::AddNode {
            id: "node-1".into(),
            slot: "dev".into(),
            position: at(0., 0.),
        }
    );
    assert!(editor.apply(edit));
    assert_eq!(editor.unused_node_id(), "node-2");
    assert!(!editor.apply(Edit::AddNode {
        id: "plan".into(),
        slot: "dev".into(),
        position: at(0., 0.),
    }));
}

#[test]
fn removing_a_node_removes_it_from_dependencies_and_layout() {
    let mut editor = editor();

    assert!(editor.apply(Edit::RemoveNodes(vec!["plan".into()])));
    assert!(node(&editor, "review").needs.is_empty());
    assert!(editor.definition().layout.is_empty());

    // The template still reads the removed node; validation reports it.
    assert!(editor.errors().iter().any(|error| matches!(
        error,
        DefinitionError::NotAncestor { reference, .. } if reference == "plan"
    )));
}

#[test]
fn connecting_appends_once_and_disconnecting_removes() {
    let mut editor = editor();

    assert!(editor.apply(Edit::AddNode {
        id: "test".into(),
        slot: "dev".into(),
        position: at(0., 0.),
    }));
    assert!(editor.apply(Edit::Connect {
        from: "test".into(),
        to: "review".into(),
    }));
    assert_eq!(node(&editor, "review").needs, ["plan", "test"]);

    assert!(!editor.apply(Edit::Connect {
        from: "test".into(),
        to: "review".into(),
    }));
    assert!(!editor.apply(Edit::Connect {
        from: "missing".into(),
        to: "review".into(),
    }));

    assert!(editor.apply(Edit::Disconnect {
        from: "plan".into(),
        to: "review".into(),
    }));
    assert_eq!(node(&editor, "review").needs, ["test"]);
}

#[test]
fn a_cycle_is_allowed_and_reported() {
    let mut editor = editor();

    assert!(editor.apply(Edit::Connect {
        from: "review".into(),
        to: "plan".into(),
    }));
    assert!(
        editor
            .errors()
            .iter()
            .any(|error| matches!(error, DefinitionError::Cycle(_)))
    );
}

#[test]
fn renaming_a_node_follows_dependencies_layout_and_templates() {
    let mut editor = editor();

    assert!(editor.apply(Edit::RenameNode {
        from: "plan".into(),
        to: "outline".into(),
    }));

    let review = node(&editor, "review");

    assert_eq!(review.needs, ["outline"]);
    assert_eq!(
        review.prompt.as_deref(),
        Some("Check {{outline.output}}, not \\{{plan.output}}")
    );
    assert!(editor.definition().layout.contains_key("outline"));
    assert!(editor.errors().is_empty());

    assert!(!editor.apply(Edit::RenameNode {
        from: "outline".into(),
        to: "review".into(),
    }));
}

#[test]
fn slots_are_added_renamed_set_and_removed() {
    let mut editor = editor();

    assert!(editor.apply(Edit::AddSlot {
        name: "web".into(),
        profile: ProfileReference {
            kind: AgentKind::Codex,
            name: "Web".into(),
        },
    }));
    assert!(!editor.apply(Edit::AddSlot {
        name: "web".into(),
        profile: ProfileReference {
            kind: AgentKind::Codex,
            name: "Web".into(),
        },
    }));

    assert!(editor.apply(Edit::RenameSlot {
        from: "dev".into(),
        to: "builder".into(),
    }));
    assert_eq!(node(&editor, "plan").slot, "builder");

    let body = SlotBody {
        profile: ProfileReference {
            kind: AgentKind::Claude,
            name: "Other".into(),
        },
        role: "You build.".into(),
        settings: ThreadSettings {
            model: Some("opus".into()),
            ..ThreadSettings::default()
        },
    };

    assert!(editor.apply(Edit::SetSlot {
        name: "builder".into(),
        body: body.clone(),
    }));
    assert_eq!(editor.definition().slots[0].body, body);

    assert!(editor.apply(Edit::RemoveSlot("critic".into())));
    assert!(editor.errors().iter().any(|error| matches!(
        error,
        DefinitionError::UnknownSlot { node, .. } if node == "review"
    )));
}

#[test]
fn the_parallelism_limit_is_set_and_cleared() {
    let mut editor = editor();

    assert!(editor.apply(Edit::SetMaxParallel(Some(2))));
    assert_eq!(editor.definition().max_parallel, Some(2));
    assert!(editor.apply(Edit::SetMaxParallel(None)));
    assert_eq!(editor.definition().max_parallel, None);
}

#[test]
fn undo_and_redo_walk_the_history_and_track_unsaved_edits() {
    let mut editor = editor();

    assert!(!editor.is_dirty());
    assert!(!editor.undo());

    assert!(editor.apply(Edit::RemoveNodes(vec!["plan".into()])));
    assert!(editor.is_dirty());

    assert!(editor.undo());
    assert!(!editor.is_dirty(), "undoing to the saved version is clean");
    assert!(editor.definition().layout.contains_key("plan"));
    assert_eq!(node(&editor, "review").needs, ["plan"]);

    assert!(editor.redo());
    assert!(editor.is_dirty());

    editor.mark_saved();

    assert!(!editor.is_dirty());
    assert!(editor.can_undo(), "saving keeps the history");

    assert!(editor.undo());
    assert!(editor.is_dirty());

    // A new edit after undoing drops what could have been redone.
    assert!(editor.apply(Edit::SetMaxParallel(Some(4))));
    assert!(!editor.can_redo());
}

#[test]
fn an_edit_that_changes_nothing_stays_out_of_the_history() {
    let mut editor = editor();

    assert!(!editor.apply(Edit::MoveNodes(vec![("plan".into(), at(10., 20.))])));
    assert!(!editor.can_undo());
}

#[test]
fn reloading_replaces_the_definition_and_the_history() {
    let mut editor = editor();

    assert!(editor.apply(Edit::SetMaxParallel(Some(2))));

    let fresh = editor.definition().clone();

    editor.reload(fresh);

    assert!(!editor.is_dirty());
    assert!(!editor.can_undo());
}
