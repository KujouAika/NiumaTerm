//! Records the app renders. They are flat and final: the app shows them as
//! they are, and every protocol and view type is converted here, once, so
//! the desktop types never grow FFI attributes.

use std::time::Instant;

use nmt_agent::chat::Item;
use nmt_agent::session::lifecycle::Status as LifecycleStatus;
use nmt_agent::session::view::{AgentView, ViewEntry};
use nmt_remote::connection::Status;
use nmt_remote_core::push::{PushEnvironment, PushKind, PushRegistration};
use nmt_remote_core::rpc::{EndReason, HostInfo, Origin, SessionInfo, SessionKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum HostStatus {
    /// Nothing needs the host, so nothing is connected.
    Idle,
    Connecting,
    Connected,
    /// The link dropped; views wait while it comes back.
    Reconnecting,
    /// The host no longer trusts this device; pairing again is the fix.
    Refused,
    /// Connecting failed a few times in a row; the app shows the host as
    /// unreachable until the user retries or the network changes.
    Unreachable,
}

impl From<Status> for HostStatus {
    fn from(status: Status) -> Self {
        match status {
            Status::Idle => Self::Idle,
            Status::Connecting => Self::Connecting,
            Status::Connected => Self::Connected,
            Status::Reconnecting => Self::Reconnecting,
            Status::Refused => Self::Refused,
            Status::Unreachable => Self::Unreachable,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct HostRecord {
    pub id: String,
    pub name: String,
    pub status: HostStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SessionType {
    Terminal,
    Agent,
    /// A kind this build does not know, from a newer host.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SessionRecord {
    pub id: String,
    pub title: String,
    pub kind: SessionType,

    /// The agent an agent session runs, as `host.info` names harnesses.
    pub harness: Option<String>,

    /// Started from a device rather than as a tab at the host.
    pub remote_origin: bool,
}

impl From<SessionInfo> for SessionRecord {
    fn from(info: SessionInfo) -> Self {
        Self {
            id: info.session,
            title: info.title,
            kind: match info.kind {
                SessionKind::Terminal => SessionType::Terminal,
                SessionKind::Agent => SessionType::Agent,
                SessionKind::Unknown => SessionType::Other,
            },
            harness: info.harness,
            remote_origin: info.origin == Origin::Remote,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AgentProfileRecord {
    pub name: String,
    pub harness: String,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct WorkspaceRecord {
    pub name: String,
    pub path: String,
}

/// What this device may start on a host.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct HostOffer {
    pub agents: Vec<AgentProfileRecord>,
    pub workspaces: Vec<WorkspaceRecord>,
}

impl From<HostInfo> for HostOffer {
    fn from(info: HostInfo) -> Self {
        Self {
            agents: info
                .agents
                .into_iter()
                .map(|agent| AgentProfileRecord {
                    name: agent.name,
                    harness: agent.harness,
                })
                .collect(),
            workspaces: info
                .workspaces
                .into_iter()
                .map(|workspace| WorkspaceRecord {
                    name: workspace.name,
                    path: workspace.path,
                })
                .collect(),
        }
    }
}

/// One transcript item, reduced to what a row shows.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum AgentItem {
    User {
        text: String,
        images: u32,
    },
    /// Markdown text.
    Agent {
        text: String,
    },
    Reasoning {
        summary: String,
    },
    Command {
        command: String,
        purpose: Option<String>,
        output: Option<String>,
        status: Option<String>,
        exit_code: Option<i64>,
    },
    FileChange {
        paths: String,
        diff: Option<String>,
        status: Option<String>,
    },
    Compaction {
        summary: Option<String>,
    },
    Tool {
        kind: String,
        title: String,
        output: Option<String>,
        status: Option<String>,
    },
    Error {
        text: String,
    },
    /// Something the conversation recorded that is not a message or tool
    /// call, shown as a one-line note.
    Notice {
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AgentEntry {
    /// Position in the transcript. Entries change only from a splice point
    /// to the end, so a position keeps naming the same entry until then.
    pub index: u32,

    pub turn: u64,
    pub item: AgentItem,
}

pub(crate) fn agent_entry(index: usize, entry: &ViewEntry) -> AgentEntry {
    AgentEntry {
        index: index as u32,
        turn: entry.turn,
        item: agent_item(&entry.item, entry.images.len()),
    }
}

fn agent_item(item: &Item, images: usize) -> AgentItem {
    match item.clone() {
        Item::UserMessage { text } => AgentItem::User {
            text: text.unwrap_or_default(),
            images: images as u32,
        },
        Item::AgentMessage { text, .. } => AgentItem::Agent {
            text: text.unwrap_or_default(),
        },
        Item::Reasoning { summary, .. } => AgentItem::Reasoning {
            summary: summary.unwrap_or_default(),
        },
        Item::CommandExecution {
            command,
            purpose,
            aggregated_output,
            status,
            exit_code,
            ..
        } => AgentItem::Command {
            command,
            purpose,
            output: aggregated_output,
            status,
            exit_code,
        },
        Item::FileChange {
            paths,
            diff,
            status,
            ..
        } => AgentItem::FileChange {
            paths,
            diff,
            status,
        },
        Item::Compaction { detail, .. } => AgentItem::Compaction {
            summary: detail.summary,
        },
        Item::Other {
            kind,
            title,
            output,
            status,
            ..
        } => AgentItem::Tool {
            kind,
            title,
            output,
            status,
        },
        Item::Error { text } => AgentItem::Error { text },
        Item::TaskList { .. } => AgentItem::Notice {
            text: "Task list updated".into(),
        },
        Item::SideBoundary { message, .. } => AgentItem::Notice { text: message },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AgentPhase {
    Starting,
    Idle,
    Running,
    Exited,
}

/// Why this device no longer controls the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ViewEnd {
    /// The person at the host took it back; it keeps running there, and this
    /// device may take it again.
    TakenBack,
    /// The session is gone from the host.
    Closed,
    /// The host cannot be reached any more, or no longer trusts this device.
    Unreachable,
}

impl From<EndReason> for ViewEnd {
    fn from(reason: EndReason) -> Self {
        match reason {
            EndReason::Closed => Self::Closed,
            EndReason::TakenBack | EndReason::Unknown => Self::TakenBack,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ModelChoice {
    pub model: String,
    pub display: String,

    /// Reasoning efforts the model supports; empty when it has no control.
    pub efforts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PendingApproval {
    pub description: String,

    /// An answer is on its way to the agent.
    pub submitted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct QueuedMessage {
    /// Absent until the host has accepted the message into its queue; only
    /// accepted messages can be withdrawn.
    pub id: Option<String>,

    pub text: String,
}

/// Everything besides the transcript that an agent screen shows.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AgentState {
    /// The first snapshot arrived. Until then the screen has nothing to show.
    pub attached: bool,

    pub ended: Option<ViewEnd>,

    pub phase: AgentPhase,
    pub start_failure: Option<String>,

    /// A turn is running.
    pub working: bool,

    /// How long the running turn has taken, measured on this device's clock
    /// from the host's report.
    pub working_ms: Option<u64>,

    pub output_tokens: Option<u64>,
    pub detail: Option<String>,

    pub title: Option<String>,

    pub model: Option<String>,
    pub effort: Option<String>,
    pub models: Vec<ModelChoice>,
    pub plan_mode: bool,

    pub approval: Option<PendingApproval>,

    /// Question batches waiting for answers.
    pub questions: u32,

    pub queue: Vec<QueuedMessage>,

    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
}

pub(crate) fn agent_state(
    view: Option<&AgentView>,
    turn_started: Option<Instant>,
    ended: Option<ViewEnd>,
) -> AgentState {
    let Some(view) = view else {
        return AgentState {
            attached: false,
            ended,
            phase: AgentPhase::Starting,
            start_failure: None,
            working: false,
            working_ms: None,
            output_tokens: None,
            detail: None,
            title: None,
            model: None,
            effort: None,
            models: Vec::new(),
            plan_mode: false,
            approval: None,
            questions: 0,
            queue: Vec::new(),
            context_used: None,
            context_window: None,
        };
    };

    let slots = &view.slots;
    let status = &slots.status;
    let usage = slots.usage.context_window_usage;

    AgentState {
        attached: true,
        ended,
        phase: match status.status {
            LifecycleStatus::Starting => AgentPhase::Starting,
            LifecycleStatus::Idle => AgentPhase::Idle,
            LifecycleStatus::Running => AgentPhase::Running,
            LifecycleStatus::Exited => AgentPhase::Exited,
        },
        start_failure: status.start_failure.clone(),
        working: status.active,
        working_ms: status
            .active
            .then_some(turn_started)
            .flatten()
            .map(|started| started.elapsed().as_millis() as u64),
        output_tokens: status.live.output_tokens,
        detail: status.live.detail.clone(),
        title: slots.naming.title.clone(),
        model: slots.settings.settings.model.clone(),
        effort: slots.settings.settings.effort.clone(),
        models: slots
            .settings
            .models
            .iter()
            .map(|model| ModelChoice {
                model: model.model.clone(),
                display: model.display.clone(),
                efforts: model.efforts.clone(),
            })
            .collect(),
        plan_mode: slots.settings.plan_mode,
        approval: slots
            .pending
            .approval
            .as_ref()
            .map(|approval| PendingApproval {
                description: approval.description.clone(),
                submitted: approval.submitted,
            }),
        questions: slots.pending.drafts.len() as u32,
        queue: slots
            .queue
            .prompts
            .iter()
            .map(|prompt| QueuedMessage {
                id: prompt.id.clone(),
                text: prompt.text.clone(),
            })
            .collect(),
        context_used: usage.map(|usage| usage.used_tokens()),
        context_window: usage.and_then(|usage| usage.max_tokens),
    }
}

/// An event the phone can be told about while it is away from a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NotificationKind {
    TurnFinished,
    TurnFailed,
    Approval,
    Question,
}

impl From<NotificationKind> for PushKind {
    fn from(kind: NotificationKind) -> Self {
        match kind {
            NotificationKind::TurnFinished => PushKind::TurnFinished,
            NotificationKind::TurnFailed => PushKind::TurnFailed,
            NotificationKind::Approval => PushKind::Approval,
            NotificationKind::Question => PushKind::Question,
        }
    }
}

/// Where and how a host should push to this phone.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PushSettings {
    /// The forwarder that holds the APNs key for this app build.
    pub endpoint: String,

    /// The APNs device token, hex.
    pub token: String,

    /// Production for TestFlight and App Store builds, sandbox otherwise.
    pub production: bool,

    /// The 32-byte key the host seals pushes with, base64.
    pub key: String,

    pub kinds: Vec<NotificationKind>,
}

impl From<PushSettings> for PushRegistration {
    fn from(settings: PushSettings) -> Self {
        Self {
            endpoint: settings.endpoint,
            token: settings.token,
            environment: if settings.production {
                PushEnvironment::Production
            } else {
                PushEnvironment::Sandbox
            },
            key: settings.key,
            kinds: settings.kinds.into_iter().map(PushKind::from).collect(),
        }
    }
}
