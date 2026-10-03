use std::fs;

use serde_json::json;
use tempfile::tempdir;

use crate::AgentWorkspace;
use crate::chat::Item;
use crate::orchestration::definition::Definition;
use crate::orchestration::graph::Graph;
use crate::orchestration::run::{RunRecord, RunState};
use crate::orchestration::store::{RunStore, RunStoreError, recent_runs};
use crate::snapshot_store::SnapshotError;

const EPOCH: u64 = 1;

fn graph() -> Graph {
    let definition: Definition = serde_json::from_value(json!({
        "version": 1,
        "slots": { "dev": { "profile": { "kind": "claude", "name": "Default" } } },
        "nodes": [
            { "id": "plan", "slot": "dev" },
            { "id": "implement", "slot": "dev", "needs": ["plan"] },
        ],
    }))
    .unwrap();

    Graph::new(definition).unwrap()
}

fn record(graph: &Graph, workspace: &str, started_at: u64) -> RunRecord {
    RunRecord::new(
        "review".into(),
        graph,
        "Add search".into(),
        AgentWorkspace::single(Some(workspace.into())),
        started_at,
    )
}

/// Send `node`, save its output, and record it as completed.
fn complete(store: &mut RunStore, graph: &Graph, node: usize, output: &str) {
    store.save_prompt(node, "prompt text").unwrap();

    assert!(store.update(|run| run.begin_send(node, EPOCH, 10)).unwrap());
    assert!(
        store
            .update(|run| run.accept(graph, node, EPOCH, "turn"))
            .unwrap()
    );

    store.save_output(node, output).unwrap();

    assert!(
        store
            .update(|run| run.complete(node, EPOCH, "turn", 20))
            .unwrap()
    );
}

#[test]
fn outputs_and_sent_text_survive_a_restart() {
    let directory = tempdir().unwrap();
    let graph = graph();
    let run = record(&graph, "C:/project", 0);
    let id = run.id();

    let mut store = RunStore::create(directory.path(), run).unwrap();

    complete(&mut store, &graph, 0, "the plan");
    complete(&mut store, &graph, 1, "the change");
    drop(store);

    let store = RunStore::open(directory.path(), id).unwrap();

    assert_eq!(store.run().state(), RunState::Completed);
    assert_eq!(store.output(0).unwrap().as_deref(), Some("the plan"));
    assert_eq!(store.output(1).unwrap().as_deref(), Some("the change"));
    assert_eq!(store.prompt(1).unwrap().as_deref(), Some("prompt text"));
}

#[test]
fn an_output_saved_without_its_completion_is_not_shown() {
    let directory = tempdir().unwrap();
    let graph = graph();
    let run = record(&graph, "C:/project", 0);
    let id = run.id();

    let mut store = RunStore::create(directory.path(), run).unwrap();

    assert!(store.update(|run| run.begin_send(0, EPOCH, 10)).unwrap());
    assert!(
        store
            .update(|run| run.accept(&graph, 0, EPOCH, "turn"))
            .unwrap()
    );

    store.save_output(0, "written before a crash").unwrap();

    drop(store);

    let mut store = RunStore::open(directory.path(), id).unwrap();

    assert_eq!(store.output(0).unwrap(), None);
    assert!(store.update(RunRecord::interrupt).unwrap());
    assert_eq!(store.run().state(), RunState::Interrupted);
    assert_eq!(store.prompt(0).unwrap(), None);
}

#[test]
fn a_run_open_in_one_instance_is_refused_to_another() {
    let directory = tempdir().unwrap();
    let graph = graph();
    let run = record(&graph, "C:/project", 0);
    let id = run.id();
    let _owner = RunStore::create(directory.path(), run).unwrap();

    assert!(matches!(
        RunStore::open(directory.path(), id),
        Err(RunStoreError::Snapshot(SnapshotError::Locked))
    ));
}

#[test]
fn a_saved_run_keeps_its_definition_and_input() {
    let directory = tempdir().unwrap();
    let graph = graph();

    let mut store = RunStore::create(directory.path(), record(&graph, "C:/project", 0)).unwrap();

    let other = record(&graph, "C:/project", 0);

    assert!(matches!(
        store.update(|run| *run = other),
        Err(RunStoreError::DefinitionChanged)
    ));
    assert_eq!(store.run().state(), RunState::Running);
}

#[test]
fn recent_runs_lists_the_workspace_newest_first_and_skips_damaged_runs() {
    let directory = tempdir().unwrap();
    let graph = graph();

    let older = record(&graph, "C:/project", 100);
    let newer = record(&graph, "C:/project", 200);
    let elsewhere = record(&graph, "C:/other", 300);
    let damaged = record(&graph, "C:/project", 400);
    let (older_id, newer_id, damaged_id) = (older.id(), newer.id(), damaged.id());

    for run in [older, newer, elsewhere, damaged] {
        drop(RunStore::create(directory.path(), run).unwrap());
    }

    fs::write(
        directory
            .path()
            .join("agent-orchestrations")
            .join("runs")
            .join(damaged_id.to_string())
            .join("run.json"),
        b"{\"version\":1,",
    )
    .unwrap();

    let summaries = recent_runs(directory.path(), Some("C:/project")).unwrap();

    assert_eq!(
        summaries
            .iter()
            .map(|summary| (summary.id, summary.started_at))
            .collect::<Vec<_>>(),
        [(newer_id, 200), (older_id, 100)]
    );
    assert_eq!(summaries[0].definition_name, "review");
    assert!(
        recent_runs(&directory.path().join("missing"), None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_turn_transcript_is_shown_once_the_turn_has_ended() {
    let directory = tempdir().unwrap();
    let graph = graph();
    let run = record(&graph, "C:/project", 0);
    let id = run.id();

    let mut store = RunStore::create(directory.path(), run).unwrap();

    let items = vec![
        Item::UserMessage {
            text: Some("Plan: Add search".into()),
        },
        Item::AgentMessage {
            id: "reply".into(),
            text: Some("the plan".into()),
            questions: None,
        },
    ];

    assert!(store.update(|run| run.begin_send(0, EPOCH, 10)).unwrap());

    store.save_transcript(0, &items).unwrap();

    // Saved, but the turn's end is not recorded yet.
    assert_eq!(store.transcript(0).unwrap(), None);
    assert!(
        store
            .update(|run| run.fail(0, Some(EPOCH), "error".into(), 20))
            .unwrap()
    );

    drop(store);

    let mut store = RunStore::open(directory.path(), id).unwrap();

    assert_eq!(store.transcript(0).unwrap(), Some(items));

    // Resuming sends the node again; the earlier attempt's file is hidden.
    assert!(store.update(RunRecord::resume).unwrap());
    assert_eq!(store.transcript(0).unwrap(), None);
}
