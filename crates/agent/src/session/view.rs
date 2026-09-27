//! A serializable projection of one conversation, for views that run in
//! another process than the session.
//!
//! The host keeps running the backend and the [`SessionController`]; a view
//! elsewhere holds a replica controller without a backend and applies what
//! the host publishes. The projection is split into a transcript, replaced
//! from its first changed entry onwards, and slots, each replaced whole when
//! it changes. Slots are coarse because they are small: comparing and
//! resending a whole slot is cheaper to get right than diffing inside one.
//!
//! Times are sent as durations measured on the host, so the two clocks never
//! have to agree.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::chat::{
    AgentPreset, ApprovalPreset, ContextComposition, ContextWindowUsage, Item, ModelInfo, Question,
    QuestionMode, QueuedPrompt, SessionStats, SkillCatalog, SlashCommandInfo, ThreadSettings,
};
use crate::progress::{GoalStatus, TaskList};
use crate::session::commands::PendingSlashCommand;
use crate::session::controller::SessionController;
use crate::session::input::{QuestionError, QuestionKey, QuestionStatus};
use crate::session::lifecycle::Status;
use crate::transcript::TranscriptEntry;
use crate::transcript::conversation::{ConversationImage, EntryMetadata};
use crate::transcript::turns::{GenerationStats, TurnLedger};

/// Everything a view needs to render the conversation at one moment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentView {
    pub transcript: Vec<ViewEntry>,
    pub slots: ViewSlots,
}

/// One transcript entry. Image bytes travel separately, by reference, so an
/// entry resent while its text streams does not resend its images.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewEntry {
    pub turn: u64,
    pub item: Item,
    pub at: Option<i64>,
    pub images: Vec<ImageRef>,
}

impl ViewEntry {
    /// The entry as a replica's transcript holds it, with its images once
    /// the view has them.
    pub fn into_entry(self, images: Vec<Arc<ConversationImage>>) -> TranscriptEntry<EntryMetadata> {
        TranscriptEntry {
            turn: self.turn,
            item: self.item,
            metadata: EntryMetadata {
                at: self.at,
                images,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    pub id: Uuid,
    pub len: u64,
}

/// A time on the host, sent as how long ago it was. Two readings compare
/// equal when both are set or both are not: the value advances on its own,
/// and only its appearance or removal is a change worth publishing.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Since(pub Option<Duration>);

impl Since {
    pub fn of(at: Option<Instant>) -> Self {
        Self(at.map(|at| at.elapsed()))
    }

    /// The same moment on this machine's clock.
    pub fn instant(self) -> Option<Instant> {
        let now = Instant::now();

        self.0.map(|ago| now.checked_sub(ago).unwrap_or(now))
    }
}

impl PartialEq for Since {
    fn eq(&self, other: &Self) -> bool {
        self.0.is_some() == other.0.is_some()
    }
}

/// The running turn, as [`crate::transcript::turns::LiveTurn`] holds it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LiveView {
    pub started: Since,
    pub output_tokens: Option<u64>,
    pub detail: Option<String>,
    pub compacting: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusView {
    pub status: Status,
    pub epoch: u64,
    pub start_failure: Option<String>,
    pub turn: u64,
    pub active: bool,
    pub live: LiveView,
    pub submitted_at: Since,
    pub first_output_latency: Option<Duration>,
    pub last_response_at: Since,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageView {
    pub context_window_usage: Option<ContextWindowUsage>,
    pub context_composition: Option<ContextComposition>,
    pub session_stats: Option<SessionStats>,
    pub generation: GenerationStats,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingsView {
    pub settings: ThreadSettings,
    pub models: Vec<ModelInfo>,
    pub approval_presets: Vec<ApprovalPreset>,
    pub agent_presets: Vec<AgentPreset>,
    pub plan_mode: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogView {
    pub commands: Option<Vec<SlashCommandInfo>>,
    pub skills: Option<SkillCatalog>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalView {
    pub description: String,
    pub submitted: bool,
}

/// One question batch with the answers typed into it so far.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DraftView {
    pub id: String,
    pub questions: Vec<Question>,
    pub mode: QuestionMode,
    pub status: QuestionStatus,
    pub error: Option<QuestionError>,
    pub selected: Vec<Vec<usize>>,
    pub text: Vec<String>,
    pub custom: Vec<bool>,
    pub key: QuestionKey,
    pub started: Since,
    pub touched: bool,
}

/// What the conversation waits on the user for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PendingView {
    pub epoch: u64,
    pub sequence: u64,
    pub disconnected: bool,
    pub approval: Option<ApprovalView>,
    pub drafts: Vec<DraftView>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QueueView {
    pub prompts: VecDeque<QueuedPrompt>,
    pub commands: VecDeque<PendingSlashCommand>,
    pub awaiting_turn: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TasksView {
    pub list: Option<TaskList>,
    pub snapshots: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSlots {
    pub status: StatusView,
    pub usage: UsageView,
    pub turns: TurnLedger,
    pub settings: SettingsView,
    pub catalogs: CatalogView,
    pub pending: PendingView,
    pub queue: QueueView,
    pub goal: Option<GoalStatus>,
    pub tasks: TasksView,

    /// Whether the conversation has its title, so a prompt from any view
    /// knows not to name it again.
    pub named: bool,
}

/// One slot, replacing its previous value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "key", content = "value", rename_all = "snake_case")]
pub enum ViewSlot {
    Status(StatusView),
    Usage(UsageView),
    Turns(TurnLedger),
    Settings(SettingsView),
    Catalogs(CatalogView),
    Pending(PendingView),
    Queue(QueueView),
    Goal(Option<GoalStatus>),
    Tasks(TasksView),
    Named(bool),
}

impl ViewSlots {
    /// Every slot, for a view starting from nothing.
    pub fn into_slots(self) -> Vec<ViewSlot> {
        vec![
            ViewSlot::Status(self.status),
            ViewSlot::Usage(self.usage),
            ViewSlot::Turns(self.turns),
            ViewSlot::Settings(self.settings),
            ViewSlot::Catalogs(self.catalogs),
            ViewSlot::Pending(self.pending),
            ViewSlot::Queue(self.queue),
            ViewSlot::Goal(self.goal),
            ViewSlot::Tasks(self.tasks),
            ViewSlot::Named(self.named),
        ]
    }

    /// The slots that differ from `previous`.
    fn changed_since(&self, previous: &Self) -> Vec<ViewSlot> {
        let mut changed = Vec::new();

        if self.status != previous.status {
            changed.push(ViewSlot::Status(self.status.clone()));
        }

        if self.usage != previous.usage {
            changed.push(ViewSlot::Usage(self.usage.clone()));
        }

        if self.turns != previous.turns {
            changed.push(ViewSlot::Turns(self.turns.clone()));
        }

        if self.settings != previous.settings {
            changed.push(ViewSlot::Settings(self.settings.clone()));
        }

        if self.catalogs != previous.catalogs {
            changed.push(ViewSlot::Catalogs(self.catalogs.clone()));
        }

        if self.pending != previous.pending {
            changed.push(ViewSlot::Pending(self.pending.clone()));
        }

        if self.queue != previous.queue {
            changed.push(ViewSlot::Queue(self.queue.clone()));
        }

        if self.goal != previous.goal {
            changed.push(ViewSlot::Goal(self.goal.clone()));
        }

        if self.tasks != previous.tasks {
            changed.push(ViewSlot::Tasks(self.tasks.clone()));
        }

        if self.named != previous.named {
            changed.push(ViewSlot::Named(self.named));
        }

        changed
    }
}

/// A change to a view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ViewOp {
    /// Replace every entry from `from` to the end. One rule covers new
    /// entries, streamed text, completions, and a cleared or trimmed
    /// transcript.
    Splice {
        from: usize,
        entries: Vec<ViewEntry>,
    },
    Slot {
        slot: Box<ViewSlot>,
    },
}

/// Turns a controller's state into view operations, remembering what it
/// published last so only changes go out.
#[derive(Default)]
pub struct ViewPublisher {
    version: (u64, u64),
    slots: Option<ViewSlots>,
}

impl ViewPublisher {
    /// The whole view, which later [`Self::changes`] build on.
    pub fn snapshot(&mut self, controller: &SessionController) -> AgentView {
        let slots = controller.view_slots();

        self.version = controller.conversation().borrow().version();
        self.slots = Some(slots.clone());

        AgentView {
            transcript: controller.transcript_view(0),
            slots,
        }
    }

    /// What changed since the last snapshot or changes.
    pub fn changes(&mut self, controller: &SessionController) -> Vec<ViewOp> {
        let mut ops = Vec::new();

        let change = controller
            .conversation()
            .borrow()
            .changes_since(self.version);

        if let Some(change) = change {
            let conversation = controller.conversation().borrow();
            let from = change.first.min(conversation.content.entries().len());

            self.version = conversation.version();

            drop(conversation);

            ops.push(ViewOp::Splice {
                from,
                entries: controller.transcript_view(from),
            });
        }

        let slots = controller.view_slots();

        match &self.slots {
            Some(previous) => {
                ops.extend(
                    slots
                        .changed_since(previous)
                        .into_iter()
                        .map(|slot| ViewOp::Slot {
                            slot: Box::new(slot),
                        }),
                )
            }
            None => ops.extend(
                slots
                    .clone()
                    .into_slots()
                    .into_iter()
                    .map(|slot| ViewOp::Slot {
                        slot: Box::new(slot),
                    }),
            ),
        }

        self.slots = Some(slots);

        ops
    }
}
