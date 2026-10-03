use serde_json::{Value, json};

use crate::orchestration::definition::Definition;
use crate::orchestration::graph::Graph;
use crate::orchestration::run::{NodeState, RunRecord, RunState};
use crate::orchestration::schedule::{Dispatch, schedule};

const EPOCH: u64 = 1;

fn graph(slots: &[&str], max_parallel: u32, nodes: Value) -> Graph {
    let slots: serde_json::Map<String, Value> = slots
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                json!({ "profile": { "kind": "claude", "name": "Default" } }),
            )
        })
        .collect();

    let definition: Definition = serde_json::from_value(json!({
        "version": 1,
        "max_parallel": max_parallel,
        "slots": slots,
        "nodes": nodes,
    }))
    .unwrap();

    Graph::new(definition).unwrap()
}

/// `plan` on `dev`, then `frontend` on `web` and `backend` on `api` in
/// parallel, then `review` on `dev` after both.
fn diamond() -> Graph {
    graph(
        &["dev", "web", "api"],
        3,
        json!([
            { "id": "plan", "slot": "dev" },
            { "id": "frontend", "slot": "web", "needs": ["plan"] },
            { "id": "backend", "slot": "api", "needs": ["plan"] },
            { "id": "review", "slot": "dev", "needs": ["frontend", "backend"] },
        ]),
    )
}

fn run(graph: &Graph) -> RunRecord {
    RunRecord::new("diamond".into(), graph, "Add search".into(), None, 0)
}

fn all_ready(graph: &Graph) -> Vec<bool> {
    vec![true; graph.slot_count()]
}

fn send(record: &mut RunRecord, graph: &Graph, node: usize) {
    assert!(record.begin_send(node, EPOCH, 10));
    assert!(record.accept(graph, node, EPOCH, &format!("turn-{node}")));
}

fn finish(record: &mut RunRecord, node: usize) {
    assert!(record.complete(node, EPOCH, &format!("turn-{node}"), 20));
}

fn states(record: &RunRecord) -> Vec<NodeState> {
    record
        .nodes()
        .iter()
        .map(|node| node.state.clone())
        .collect()
}

#[test]
fn independent_nodes_start_together_once_their_dependency_completes() {
    let graph = diamond();

    let mut record = run(&graph);

    assert_eq!(schedule(&graph, &record, &all_ready(&graph)).send, [0]);

    send(&mut record, &graph, 0);

    assert_eq!(
        schedule(&graph, &record, &all_ready(&graph)),
        Dispatch::default()
    );

    finish(&mut record, 0);

    assert_eq!(schedule(&graph, &record, &all_ready(&graph)).send, [1, 2]);

    send(&mut record, &graph, 1);
    send(&mut record, &graph, 2);
    finish(&mut record, 1);

    assert_eq!(
        schedule(&graph, &record, &all_ready(&graph)),
        Dispatch::default()
    );

    finish(&mut record, 2);

    assert_eq!(schedule(&graph, &record, &all_ready(&graph)).send, [3]);

    send(&mut record, &graph, 3);
    finish(&mut record, 3);

    assert_eq!(record.state(), RunState::Completed);
    assert_eq!(record.ended_at(), Some(20));
}

#[test]
fn the_parallelism_limit_queues_ready_nodes() {
    let graph = graph(
        &["a", "b", "c", "d"],
        2,
        json!([
            { "id": "one", "slot": "a" },
            { "id": "two", "slot": "b" },
            { "id": "three", "slot": "c" },
            { "id": "four", "slot": "d" },
        ]),
    );

    let mut record = run(&graph);

    assert_eq!(schedule(&graph, &record, &all_ready(&graph)).send, [0, 1]);

    send(&mut record, &graph, 0);
    send(&mut record, &graph, 1);

    assert_eq!(
        schedule(&graph, &record, &all_ready(&graph)),
        Dispatch::default()
    );

    finish(&mut record, 1);

    assert_eq!(schedule(&graph, &record, &all_ready(&graph)).send, [2]);
}

#[test]
fn a_slot_that_is_not_ready_is_prepared_within_the_limit() {
    let graph = diamond();

    let mut record = run(&graph);

    send(&mut record, &graph, 0);
    finish(&mut record, 0);

    assert_eq!(
        schedule(&graph, &record, &[true, false, true]),
        Dispatch {
            send: vec![2],
            prepare: vec![1],
        }
    );
}

#[test]
fn a_failure_lets_running_siblings_finish_and_starts_nothing_else() {
    let graph = diamond();

    let mut record = run(&graph);

    send(&mut record, &graph, 0);
    finish(&mut record, 0);
    send(&mut record, &graph, 1);
    send(&mut record, &graph, 2);

    assert!(record.fail(1, Some(EPOCH), "turn ended with an error".into(), 30));
    assert_eq!(record.state(), RunState::Failing);
    assert_eq!(
        schedule(&graph, &record, &all_ready(&graph)),
        Dispatch::default()
    );

    finish(&mut record, 2);

    assert_eq!(record.state(), RunState::Failed);
    assert_eq!(
        states(&record),
        [
            NodeState::Completed {
                turn: "turn-0".into()
            },
            NodeState::Failed {
                reason: "turn ended with an error".into()
            },
            NodeState::Completed {
                turn: "turn-2".into()
            },
            NodeState::NotRun,
        ]
    );
}

#[test]
fn a_session_that_cannot_start_fails_its_waiting_node() {
    let graph = diamond();

    let mut record = run(&graph);

    assert!(record.fail(0, None, "profile is missing".into(), 5));
    assert_eq!(record.state(), RunState::Failed);
    assert!(!record.fail(1, None, "late".into(), 6));
}

#[test]
fn stopping_returns_the_running_turns_to_interrupt() {
    let graph = diamond();

    let mut record = run(&graph);

    send(&mut record, &graph, 0);
    finish(&mut record, 0);
    send(&mut record, &graph, 1);

    assert_eq!(record.stop(40), [1]);
    assert_eq!(record.state(), RunState::Stopped);
    assert_eq!(
        states(&record)[1..],
        [NodeState::Stopped, NodeState::NotRun, NodeState::NotRun]
    );

    // The interrupted turn's own end arrives later and changes nothing.
    assert!(!record.complete(1, EPOCH, "turn-1", 41));
    assert!(record.stop(42).is_empty());
}

#[test]
fn a_reopened_run_is_interrupted_and_sends_nothing() {
    let graph = diamond();

    let mut record = run(&graph);

    send(&mut record, &graph, 0);

    assert!(record.interrupt());
    assert_eq!(record.state(), RunState::Interrupted);
    assert_eq!(record.nodes()[0].state, NodeState::Interrupted);
    assert_eq!(record.nodes()[1].state, NodeState::Waiting);
    assert_eq!(
        schedule(&graph, &record, &all_ready(&graph)),
        Dispatch::default()
    );
}

#[test]
fn resuming_keeps_completed_nodes_and_sends_the_rest_again() {
    let graph = diamond();

    let mut record = run(&graph);

    send(&mut record, &graph, 0);
    finish(&mut record, 0);
    send(&mut record, &graph, 1);
    send(&mut record, &graph, 2);
    finish(&mut record, 2);

    assert!(record.fail(1, Some(EPOCH), "error".into(), 30));
    assert!(record.resume());
    assert_eq!(record.state(), RunState::Running);
    assert_eq!(record.ended_at(), None);
    assert!(record.nodes()[0].state.is_completed());
    assert!(record.nodes()[2].state.is_completed());
    assert_eq!(schedule(&graph, &record, &all_ready(&graph)).send, [1]);
    assert!(!record.resume());
}

#[test]
fn signals_from_another_epoch_or_turn_are_ignored() {
    let graph = diamond();

    let mut record = run(&graph);

    assert!(record.begin_send(0, EPOCH, 10));
    assert!(!record.begin_send(0, EPOCH, 10));
    assert!(!record.accept(&graph, 0, EPOCH + 1, "turn-0"));
    assert!(!record.slots()[0].opened);
    assert!(record.accept(&graph, 0, EPOCH, "turn-0"));
    assert!(record.slots()[0].opened);
    assert!(!record.complete(0, EPOCH, "another-turn", 20));
    assert!(!record.fail(0, Some(EPOCH + 1), "stale".into(), 20));
    assert!(!record.fail(0, None, "stale".into(), 20));
    assert!(record.complete(0, EPOCH, "turn-0", 20));
}

#[test]
fn the_saved_form_round_trips() {
    let graph = diamond();

    let mut record = run(&graph);

    send(&mut record, &graph, 0);

    record.record_conversation(0, "thread-dev".into());

    let saved = serde_json::to_string(&record).unwrap();

    assert_eq!(serde_json::from_str::<RunRecord>(&saved).unwrap(), record);
}
