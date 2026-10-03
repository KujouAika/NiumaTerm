//! One Team member's live agent session and the Team work assigned to it.

use gpui::{App, Subscription};
use nmt_agent::chat::{TeamDecisionRequest, ThreadSettings};
use nmt_agent::session::RecoveryIdentity;
use nmt_agent::team::model::AttemptId;

use crate::agent_tab::execution::{SessionOwner, WorkStatus};

pub(super) struct MemberHost {
    pub(super) owner: SessionOwner,

    /// The attempt this member was last sent and has not finished.
    pub(super) active: Option<AttemptId>,

    /// The backend epoch the room last recorded this member ready for.
    pub(super) ready_epoch: Option<u64>,

    _subscriptions: Vec<Subscription>,
}

impl MemberHost {
    pub(super) fn new(owner: SessionOwner, subscriptions: Vec<Subscription>) -> Self {
        Self {
            owner,
            active: None,
            ready_epoch: None,
            _subscriptions: subscriptions,
        }
    }

    /// Whether the member still has Team work in flight or its session is
    /// doing anything at all, either of which rules out sending it more.
    pub(super) fn is_busy(&self, cx: &App) -> bool {
        self.active.is_some()
            || self.owner.session().read(cx).work_status() != WorkStatus::default()
    }

    /// Start the member's session, resuming `recovery` when it has one.
    pub(super) fn start(&self, recovery: Option<RecoveryIdentity>, cx: &mut App) {
        self.owner.session().update(cx, |session, cx| {
            session.start(recovery, true, |_, _| {}, cx);
        });
    }

    /// Make `settings` the settings the member's next turn runs with. The
    /// session also keeps them as its own, so a conversation it opens later
    /// starts on the room's values instead of back on the profile's.
    pub(super) fn apply_settings(&self, settings: ThreadSettings, cx: &mut App) {
        self.owner.session().update(cx, |session, cx| {
            session
                .controller
                .borrow_mut()
                .controls
                .set_settings(settings.clone());

            session.remember_settings(settings);

            cx.notify();
        });
    }

    pub(super) fn interrupt(&self, cx: &mut App) {
        self.owner.session().update(cx, |session, cx| {
            session.controller.borrow_mut().interrupt_from_user();

            cx.notify();
        });
    }

    /// Tell the moderator whether the room saved the decision it requested.
    pub(super) fn respond_decision(
        &self,
        request: &TeamDecisionRequest,
        accepted: bool,
        cx: &mut App,
    ) {
        let explanation = if accepted {
            "The decision is saved. It will run after this moderator turn finishes."
        } else {
            "The decision was rejected. The discussion is paused for user review."
        };

        self.owner.session().update(cx, |session, cx| {
            session
                .controller
                .borrow_mut()
                .respond_team_decision(request, accepted, explanation);

            cx.notify();
        });
    }
}
