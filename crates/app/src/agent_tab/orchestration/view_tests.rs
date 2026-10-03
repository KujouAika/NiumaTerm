use gpui::{
    AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds,
    WindowOptions, point, px, size,
};
use gpui_component::Root;
use nmt_agent::AgentWorkspace;
use nmt_agent::chat::{
    Event, Question, QuestionInput, QuestionMode, QuestionRequest, SendOutcome,
    SlashCommandOutcome, ThreadSettings,
};
use nmt_agent::orchestration::definition::Definition;
use nmt_agent::orchestration::graph::Graph;
use nmt_agent::session::test_support::TestBackend;
use nmt_agent::session::{AgentKind, Backend};
use nmt_config::profile::AgentProfile;
use serde_json::json;
use tempfile::{TempDir, tempdir};

use crate::agent_tab::execution::AgentSession;
use crate::agent_tab::orchestration::view::SIDEBAR_WIDTH;
use crate::agent_tab::orchestration::{OrchestrationPane, OrchestrationRuntime};
use crate::agent_tab::settings::AgentSettings;

struct Fixture {
    _directory: TempDir,
    runtime: Entity<OrchestrationRuntime>,
    session: Entity<AgentSession>,
    pane: Entity<OrchestrationPane>,
}

/// A window `width` pixels wide whose Orchestration pane shows a one-node run
/// that has been sent to a slot backed by a test session.
fn running_node(width: f32, cx: &mut TestAppContext) -> (Fixture, VisualTestContext) {
    let directory = tempdir().unwrap();

    let definition: Definition = serde_json::from_value(json!({
        "version": 1,
        "slots": { "dev": { "profile": { "kind": "codex", "name": "test" } } },
        "nodes": [{ "id": "plan", "slot": "dev", "prompt": "Plan it." }],
    }))
    .unwrap();

    let (runtime, session, pane, window) = cx.update(|cx| {
        gpui_component::init(cx);

        cx.set_global(AgentSettings::default());

        let runtime = OrchestrationRuntime::start(
            directory.path(),
            "plan".into(),
            Graph::new(definition).unwrap(),
            String::new(),
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
            runtime.attach_slot_owner(0, owner, AgentKind::Codex, None, cx)
        });

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(width), px(700.)),
            ))),
            ..WindowOptions::default()
        };

        let mut pane = None;

        let window = cx
            .open_window(options, |window, cx| {
                let view = cx.new(|cx| {
                    OrchestrationPane::new(
                        directory.path().to_owned(),
                        AgentWorkspace::default(),
                        None,
                        window,
                        cx,
                    )
                });

                pane = Some(view.clone());

                cx.new(|cx| Root::new(view, window, cx))
            })
            .unwrap();

        (runtime, session, pane.unwrap(), window)
    });

    let mut cx = VisualTestContext::from_window(window.into(), cx);

    session.update(&mut cx, |session, cx| {
        let backend = TestBackend::new(
            [SendOutcome::StartedTurn],
            SlashCommandOutcome::NotReady,
            vec![],
        );

        let epoch = session.controller.borrow_mut().starting(None);

        session.install(Ok(Backend::Test(backend)), epoch, "test", cx);

        session.on_event(epoch, Event::Ready(ThreadSettings::default()), cx);
    });

    cx.run_until_parked();

    let fixture = Fixture {
        _directory: directory,
        runtime,
        session,
        pane,
    };

    (fixture, cx)
}

/// Deliver `event` to the slot session, then open the node's details.
fn ask_and_open_details(fixture: &Fixture, event: Event, cx: &mut VisualTestContext) {
    fixture.session.update(cx, |session, cx| {
        let epoch = session.controller.borrow().runtime().epoch();

        session.on_event(epoch, event, cx);
    });

    cx.run_until_parked();

    assert!(
        fixture
            .runtime
            .read_with(cx, |runtime, cx| runtime.needs_input(0, cx))
    );

    cx.update(|window, cx| {
        fixture.pane.update(cx, |pane, cx| {
            pane.show_runtime(fixture.runtime.clone(), cx);
            pane.open_detail(0, cx);
        });

        let _ = window.draw(cx);
    });
}

#[gpui::test]
async fn a_waiting_node_draws_its_question_in_its_details(cx: &mut TestAppContext) {
    let (fixture, mut cx) = running_node(1400., cx);

    ask_and_open_details(
        &fixture,
        Event::InputRequested(QuestionRequest {
            id: "scope".into(),
            mode: QuestionMode::Blocking,
            questions: vec![Question {
                input: QuestionInput::Text,
                header: None,
                question: "How far should this go?".into(),
                multi_select: false,
                options: Vec::new(),
            }],
        }),
        &mut cx,
    );

    assert!(
        cx.debug_bounds("agent-question-panel").is_some(),
        "the node's question is drawn in its details"
    );
}

#[gpui::test]
async fn an_approval_in_a_narrow_pane_keeps_every_button_inside_it(cx: &mut TestAppContext) {
    let (fixture, mut cx) = running_node(900., cx);

    ask_and_open_details(
        &fixture,
        Event::ApprovalRequested {
            description: "Edit file: C:/Workspace/project/src/main.rs".into(),
        },
        &mut cx,
    );

    let surface = cx.debug_bounds("orchestration-surface").unwrap();
    let cancel = cx.debug_bounds("approval-cancel").unwrap();

    assert!(
        cancel.left() >= surface.left() + px(SIDEBAR_WIDTH),
        "the first approval button stays right of the definitions list: {cancel:?}"
    );
    assert!(cancel.right() <= surface.right());
}
