use std::path::Path;

use gpui::{Entity, TestAppContext};
use nmt_agent::AgentWorkspace;
use nmt_agent::chat::{Event, Item, SendOutcome, SlashCommandOutcome, ThreadSettings};
use nmt_agent::orchestration::definition::Definition;
use nmt_agent::orchestration::graph::Graph;
use nmt_agent::orchestration::run::{NodeState, RunRecord, RunState};
use nmt_agent::orchestration::store::RunStore;
use nmt_agent::session::test_support::TestBackend;
use nmt_agent::session::{AgentKind, Backend};
use nmt_config::profile::AgentProfile;
use serde_json::json;
use tempfile::tempdir;

use crate::agent_tab::execution::AgentSession;
use crate::agent_tab::orchestration::OrchestrationRuntime;
use crate::agent_tab::settings::AgentSettings;

/// `plan` then `implement` on slot `dev`, whose role opens the conversation.
fn chain() -> Graph {
    let definition: Definition = serde_json::from_value(json!({
        "version": 1,
        "slots": {
            "dev": {
                "profile": { "kind": "codex", "name": "test" },
                "role": "You implement changes.",
            },
        },
        "nodes": [
            { "id": "plan", "slot": "dev", "prompt": "Plan: {{input}}" },
            { "id": "implement", "slot": "dev", "needs": ["plan"] },
        ],
    }))
    .unwrap();

    Graph::new(definition).unwrap()
}

/// Start a run of `graph` whose slot 0 is a session with a test backend
/// that accepts `sends` prompts and reports the conversation `slot-thread`.
/// `resumed` is the conversation the slot session was started to resume.
/// Returns the runtime, the slot session and its epoch.
fn start(
    directory: &Path,
    graph: Graph,
    sends: usize,
    resumed: Option<&str>,
    cx: &mut TestAppContext,
) -> (Entity<OrchestrationRuntime>, Entity<AgentSession>, u64) {
    let (runtime, session) = cx.update(|cx| {
        gpui_component::init(cx);

        cx.set_global(AgentSettings::default());

        let runtime = OrchestrationRuntime::start(
            directory,
            "chain".into(),
            graph,
            "Add search".into(),
            AgentWorkspace::default(),
            cx,
        );

        let owner = AgentSession::create(
            AgentProfile {
                name: "test".into(),
                kind: AgentKind::Codex,
                ..AgentProfile::default()
            },
            AgentWorkspace::default(),
            None,
            cx,
        );

        let session = owner.session().clone();

        runtime.update(cx, |runtime, cx| {
            runtime.attach_slot_owner(0, owner, AgentKind::Codex, resumed.map(str::to_owned), cx)
        });

        (runtime, session)
    });

    let epoch = session.update(cx, |session, cx| {
        let backend = TestBackend::new(
            vec![SendOutcome::StartedTurn; sends],
            SlashCommandOutcome::NotReady,
            vec![],
        )
        .with_recovery(AgentKind::Codex, "slot-thread");

        let epoch = session.controller.borrow_mut().starting(None);

        assert_eq!(
            session.install(Ok(Backend::Test(backend)), epoch, "test", cx),
            Some(true)
        );

        session.on_event(epoch, Event::Ready(ThreadSettings::default()), cx);

        epoch
    });

    cx.run_until_parked();

    (runtime, session, epoch)
}

/// Play one provider turn that answers `reply`, or ends with `error`.
fn answer(
    session: &Entity<AgentSession>,
    epoch: u64,
    turn: &str,
    reply: &str,
    error: Option<&str>,
    cx: &mut TestAppContext,
) {
    session.update(cx, |session, cx| {
        session.on_event(epoch, Event::ProviderTurnAccepted { id: turn.into() }, cx);
        session.on_event(epoch, Event::TurnStarted, cx);

        session.on_event(
            epoch,
            Event::ItemStarted(Item::AgentMessage {
                id: format!("{turn}-reply"),
                text: Some(String::new()),
                questions: None,
            }),
            cx,
        );

        session.on_event(
            epoch,
            Event::AgentMessageDelta {
                item_id: format!("{turn}-reply"),
                delta: reply.into(),
            },
            cx,
        );

        session.on_event(
            epoch,
            Event::TurnCompleted {
                error: error.map(str::to_owned),
            },
            cx,
        );

        session.on_event(
            epoch,
            Event::ProviderTurnFinished {
                id: turn.into(),
                error: error.map(str::to_owned),
            },
            cx,
        );
    });

    cx.run_until_parked();
}

fn node_states(runtime: &Entity<OrchestrationRuntime>, cx: &mut TestAppContext) -> Vec<NodeState> {
    runtime.read_with(cx, |runtime, _| {
        runtime
            .run()
            .unwrap()
            .nodes()
            .iter()
            .map(|node| node.state.clone())
            .collect()
    })
}

fn run_state(runtime: &Entity<OrchestrationRuntime>, cx: &mut TestAppContext) -> RunState {
    runtime.read_with(cx, |runtime, _| runtime.run().unwrap().state())
}

/// Close the runtime and read the saved run back from disk.
fn saved(
    directory: &Path,
    runtime: Entity<OrchestrationRuntime>,
    cx: &mut TestAppContext,
) -> RunStore {
    let id = runtime.read_with(cx, |runtime, _| runtime.run().unwrap().id());

    runtime.update(cx, |runtime, cx| runtime.close(cx));

    drop(runtime);

    cx.update(|_| {});
    cx.run_until_parked();

    RunStore::open(directory, id).unwrap()
}

#[gpui::test]
async fn nodes_on_one_slot_run_in_order_in_one_conversation(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let (runtime, session, epoch) = start(directory.path(), chain(), 2, None, cx);

    assert!(matches!(
        node_states(&runtime, cx)[0],
        NodeState::Sending { .. }
    ));

    answer(&session, epoch, "turn-plan", "the plan", None, cx);

    assert!(node_states(&runtime, cx)[0].is_completed());
    assert!(matches!(
        node_states(&runtime, cx)[1],
        NodeState::Sending { .. }
    ));

    answer(&session, epoch, "turn-implement", "the change", None, cx);

    assert_eq!(run_state(&runtime, cx), RunState::Completed);

    let store = saved(directory.path(), runtime, cx);

    assert_eq!(
        store.prompt(0).unwrap().as_deref(),
        Some("You implement changes.\n\nPlan: Add search")
    );
    assert_eq!(
        store.prompt(1).unwrap().as_deref(),
        Some("## plan\n\nthe plan")
    );
    assert_eq!(store.output(1).unwrap().as_deref(), Some("the change"));
    assert_eq!(
        store.run().slots()[0].conversation.as_deref(),
        Some("slot-thread")
    );

    let transcript = store.transcript(1).unwrap().unwrap();

    assert_eq!(
        transcript[0],
        Item::UserMessage {
            text: Some("## plan\n\nthe plan".into())
        }
    );
    assert!(
        transcript.iter().any(|item| matches!(item, Item::AgentMessage { text: Some(text), .. } if text == "the change")),
        "{transcript:?}"
    );
    assert!(
        !transcript.iter().any(
            |item| matches!(item, Item::AgentMessage { text: Some(text), .. } if text == "the plan")
        ),
        "a node's transcript holds only its own turn"
    );
}

#[gpui::test]
async fn a_failed_turn_fails_the_run(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let (runtime, session, epoch) = start(directory.path(), chain(), 2, None, cx);

    answer(
        &session,
        epoch,
        "turn-plan",
        "partial",
        Some("rate limited"),
        cx,
    );

    assert_eq!(run_state(&runtime, cx), RunState::Failed);
    assert_eq!(
        node_states(&runtime, cx),
        [
            NodeState::Failed {
                reason: "rate limited".into()
            },
            NodeState::NotRun
        ]
    );
}

#[gpui::test]
async fn stopping_interrupts_the_running_turn_and_keeps_its_transcript(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let (runtime, session, epoch) = start(directory.path(), chain(), 2, None, cx);

    session.update(cx, |session, cx| {
        session.on_event(
            epoch,
            Event::ProviderTurnAccepted {
                id: "turn-plan".into(),
            },
            cx,
        );
    });

    cx.run_until_parked();

    let stopped = runtime.update(cx, |runtime, cx| runtime.stop(cx));

    cx.run_until_parked();

    assert_eq!(stopped.await.unwrap(), [0]);
    assert_eq!(run_state(&runtime, cx), RunState::Stopped);
    assert_eq!(
        node_states(&runtime, cx),
        [NodeState::Stopped, NodeState::NotRun]
    );

    let store = saved(directory.path(), runtime, cx);

    assert_eq!(
        store.transcript(0).unwrap().unwrap()[0],
        Item::UserMessage {
            text: Some("You implement changes.\n\nPlan: Add search".into())
        }
    );
}

#[gpui::test]
async fn a_reopened_run_is_interrupted_and_sends_nothing(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let graph = chain();

    let record = RunRecord::new(
        "chain".into(),
        &graph,
        "Add search".into(),
        AgentWorkspace::default(),
        0,
    );

    let id = record.id();

    let mut store = RunStore::create(directory.path(), record).unwrap();

    assert!(store.update(|run| run.begin_send(0, 1, 10)).unwrap());

    drop(store);

    let runtime = cx.update(|cx| {
        cx.set_global(AgentSettings::default());

        OrchestrationRuntime::open(directory.path(), id, cx)
    });

    cx.run_until_parked();

    assert_eq!(run_state(&runtime, cx), RunState::Interrupted);
    assert_eq!(
        node_states(&runtime, cx),
        [NodeState::Interrupted, NodeState::Waiting]
    );
    assert!(runtime.read_with(cx, |runtime, _| runtime.resumable()));
}

#[gpui::test]
async fn a_slot_whose_conversation_cannot_be_resumed_fails_its_node(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let (runtime, _, _) = start(directory.path(), chain(), 2, Some("saved-thread"), cx);

    assert_eq!(run_state(&runtime, cx), RunState::Failed);
    assert_eq!(
        node_states(&runtime, cx)[0],
        NodeState::Failed {
            reason: "the conversation saved-thread could not be resumed".into()
        }
    );
}

#[gpui::test]
async fn resuming_with_a_missing_profile_fails_the_next_node(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let graph = chain();

    let record = RunRecord::new(
        "chain".into(),
        &graph,
        "Add search".into(),
        AgentWorkspace::default(),
        0,
    );

    let id = record.id();

    drop(RunStore::create(directory.path(), record).unwrap());

    let runtime = cx.update(|cx| {
        cx.set_global(AgentSettings::default());

        OrchestrationRuntime::open(directory.path(), id, cx)
    });

    cx.run_until_parked();

    // A run saved as going is interrupted on reopen; resuming needs the
    // slot's session, whose profile is not saved.
    let resumed = runtime.update(cx, |runtime, cx| runtime.resume(cx));

    cx.run_until_parked();

    assert!(resumed.await.unwrap());
    assert_eq!(run_state(&runtime, cx), RunState::Failed);
    assert_eq!(
        node_states(&runtime, cx)[0],
        NodeState::Failed {
            reason: "the profile `test` is not saved".into()
        }
    );
}
