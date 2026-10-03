//! One orchestration slot's live agent session and the node it is running.

use gpui::{App, Entity, Subscription};
use nmt_agent::chat::ThreadSettings;
use nmt_agent::session::lifecycle::Status;
use nmt_agent::session::{AgentKind, RecoveryIdentity};

use crate::agent_tab::execution::{AgentSession, SessionOwner, WorkStatus};

/// The node a slot is running and what its signals are matched against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LiveNode {
    pub(super) node: usize,

    /// The session epoch the node was sent under.
    pub(super) epoch: u64,

    /// The session's local turn number for the node's turn, which tags every
    /// transcript entry the turn adds.
    pub(super) turn: u64,

    /// The text sent, shown as the turn's prompt because a live echo of the
    /// user message can omit it.
    pub(super) prompt: String,
}

/// What one look at a slot's session found, for the scheduler.
pub(super) struct SlotObservation {
    pub(super) ready: bool,

    /// A conversation id to save for the slot.
    pub(super) conversation: Option<String>,

    /// Why the slot cannot take a node, when it cannot.
    pub(super) failure: Option<String>,

    /// The running node whose session exited under it.
    pub(super) exited: Option<LiveNode>,
}

pub(super) struct SlotHost {
    pub(super) owner: SessionOwner,
    pub(super) kind: AgentKind,
    pub(super) live: Option<LiveNode>,

    /// The conversation this session was started to resume. A session that
    /// reports another one has started fresh, without the slot's context.
    resumed: Option<String>,

    _subscriptions: Vec<Subscription>,
}

impl SlotHost {
    pub(super) fn new(
        owner: SessionOwner,
        kind: AgentKind,
        resumed: Option<String>,
        subscriptions: Vec<Subscription>,
    ) -> Self {
        Self {
            owner,
            kind,
            live: None,
            resumed,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn session(&self) -> &Entity<AgentSession> {
        self.owner.session()
    }

    /// Start the session with `settings`, resuming the conversation it was
    /// created for.
    pub(super) fn start(&self, settings: ThreadSettings, cx: &mut App) {
        let recovery = self
            .resumed
            .clone()
            .map(|id| RecoveryIdentity::new(self.kind, id));

        self.owner.session().update(cx, |session, cx| {
            session
                .controller
                .borrow_mut()
                .controls
                .set_settings(settings.clone());

            session.remember_settings(settings);

            cx.notify();
        });

        self.owner.start(recovery, cx);
    }

    pub(super) fn interrupt(&self, cx: &mut App) {
        self.owner.session().update(cx, |session, cx| {
            session.controller.borrow_mut().interrupt_from_user();

            cx.notify();
        });
    }

    /// Whether the agent waits for the user: an approval or answers to its
    /// questions.
    pub(super) fn needs_input(&self, cx: &App) -> bool {
        let session = self.owner.session().read(cx);
        let state = session.controller.borrow();

        state.input().waiting() || state.input().pending_count() > 0
    }

    pub(super) fn observe(&self, saved: Option<&str>, cx: &App) -> SlotObservation {
        let session = self.owner.session().read(cx);
        let work = session.work_status();
        let state = session.controller.borrow();
        let runtime = state.runtime();

        let reported = runtime
            .backend()
            .and_then(|backend| backend.recovery_identity())
            .filter(|identity| identity.kind == self.kind)
            .map(|identity| identity.id);

        let mut failure = runtime.start_failure().map(str::to_owned);
        let mut conversation = None;

        match (&self.resumed, reported) {
            (Some(expected), Some(reported)) if *expected != reported => {
                failure.get_or_insert_with(|| {
                    format!("the conversation {expected} could not be resumed")
                });
            }
            (_, Some(reported)) if saved != Some(reported.as_str()) => {
                conversation = Some(reported);
            }
            _ => {}
        }

        let exited = runtime.status() == Status::Exited;

        if exited {
            failure.get_or_insert_with(|| "the agent process exited".to_owned());
        }

        SlotObservation {
            ready: runtime.status() == Status::Idle
                && runtime.update_suspension().is_none()
                && work == WorkStatus::default()
                && self.live.is_none()
                && failure.is_none(),
            conversation,
            failure,
            exited: self.live.clone().filter(|_| exited),
        }
    }
}
