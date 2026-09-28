use std::sync::Arc;
use std::time::Instant;

use anyhow::anyhow;
use nmt_agent::session::OperationError;
use nmt_agent::session::command::{
    AgentCommand, ApplyModelSelection, Interrupt, Prompt, RenameConversation, RespondApproval,
    SubmitPrompt, SubmitRefusal, WithdrawQueuedPrompt,
};
use nmt_agent::session::controller::SubmissionBlock;
use nmt_agent::session::delivery::RecoverablePrompt;
use nmt_agent::session::input::ApprovalOutcome;
use nmt_agent::session::lifecycle::InterruptOutcome;
use nmt_agent::session::view::{AgentView, ViewOp, ViewSlot};
use nmt_platform::runtime;
use nmt_remote::connection::{AgentLink, AgentUpdate, RemoteHost};
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::AbortHandle;
use tracing::warn;

use crate::error::CoreError;
use crate::records::{AgentEntry, AgentState, ViewEnd, agent_entry, agent_state};

/// The longest title a prompt gives an unnamed conversation, matching what
/// the desktop takes from its own prompts.
const TITLE_CHARS: usize = 60;

/// Told when an agent view changed. Called on the core's threads.
#[uniffi::export(with_foreign)]
pub trait AgentObserver: Send + Sync {
    /// The view changed. `transcript_from` is the first transcript entry
    /// that changed, when any did; everything before it is unchanged.
    fn changed(&self, transcript_from: Option<u32>);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ApprovalDecision {
    Accept,
    /// Accept, and let the agent do the same again this session without
    /// asking.
    AcceptForSession,
    Decline,
    /// Decline and stop the turn.
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ApprovalResult {
    /// There was no request to answer any more.
    Ignored,
    Rejected,
    Waiting,
    Settled,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SubmitResult {
    /// The agent took the message; `started_turn` is false when it queued
    /// behind the running turn.
    Accepted {
        started_turn: bool,
    },
    /// The agent is still starting.
    NotReady,
    /// An answer to a question is still being sent.
    AnswerPending,
    /// A rewind or fork is changing the conversation.
    ConversationChanging,
    /// A slash command is starting.
    CommandStarting,
    Rejected {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct InterruptResult {
    pub accepted: bool,

    /// The message the interrupt took back before the agent answered it,
    /// for the composer to show again.
    pub restored_text: Option<String>,
}

/// A view of one agent session on a host. The host keeps the session's
/// controller; this holds the projection it publishes and sends commands.
#[derive(uniffi::Object)]
pub struct AgentHandle {
    host: Arc<RemoteHost>,
    link: Arc<AgentLink>,
    replica: Arc<Mutex<Replica>>,
    tasks: Vec<AbortHandle>,
}

#[derive(Default)]
struct Replica {
    view: Option<AgentView>,

    /// When the running turn started, on this device's clock.
    turn_started: Option<Instant>,

    /// The host went out of reach for good, or stopped trusting the device.
    unreachable: bool,
}

impl Replica {
    /// Apply one update and return the first transcript entry it changed.
    fn apply(&mut self, update: AgentUpdate) -> Option<usize> {
        match update {
            AgentUpdate::Snapshot(value) => match serde_json::from_value::<AgentView>(value) {
                Ok(view) => {
                    self.turn_started = view.slots.status.live.started.instant();
                    self.view = Some(view);

                    Some(0)
                }
                Err(error) => {
                    warn!(%error, "ignoring an agent snapshot this build cannot read");

                    None
                }
            },
            AgentUpdate::Ops(value) => {
                let ops = match serde_json::from_value::<Vec<ViewOp>>(value) {
                    Ok(ops) => ops,
                    Err(error) => {
                        warn!(%error, "ignoring agent changes this build cannot read");

                        return None;
                    }
                };

                // Changes before the snapshot describe a view this device
                // never had; the snapshot that follows replaces them.
                let view = self.view.as_mut()?;

                let mut from = None;

                for op in ops {
                    match &op {
                        ViewOp::Splice { from: at, .. } => {
                            from = Some(from.map_or(*at, |known: usize| known.min(*at)));
                        }
                        ViewOp::Slot { slot } => {
                            if let ViewSlot::Status(status) = slot.as_ref() {
                                self.turn_started = status.live.started.instant();
                            }
                        }
                    }

                    view.apply(op);
                }

                from
            }
            AgentUpdate::Ended => {
                self.unreachable = true;

                None
            }
        }
    }
}

impl AgentHandle {
    pub(crate) fn attach(
        host: Arc<RemoteHost>,
        session: String,
        observer: Arc<dyn AgentObserver>,
    ) -> Arc<Self> {
        let (link, updates) = host.agent_view(session.clone());
        let replica = Arc::new(Mutex::new(Replica::default()));

        let follow = runtime()
            .spawn(follow(updates, Arc::clone(&replica), Arc::clone(&observer)))
            .abort_handle();

        let ended = runtime()
            .spawn(watch_ended(Arc::clone(&host), observer))
            .abort_handle();

        Arc::new(Self {
            host,
            link: Arc::new(link),
            replica,
            tasks: vec![follow, ended],
        })
    }

    async fn run<C: AgentCommand>(&self, command: C) -> Result<C::Outcome, CoreError> {
        let link = Arc::clone(&self.link);
        let params = serde_json::to_value(&command)?;

        let value = runtime()
            .spawn(async move { link.call(C::METHOD, params).await })
            .await??;

        Ok(serde_json::from_value(value)?)
    }
}

#[uniffi::export]
impl AgentHandle {
    pub fn session(&self) -> String {
        self.link.session().to_owned()
    }

    /// Transcript entries from `from` to the end.
    pub fn entries(&self, from: u32) -> Vec<AgentEntry> {
        let replica = self.replica.lock();

        let Some(view) = &replica.view else {
            return Vec::new();
        };

        view.transcript
            .iter()
            .enumerate()
            .skip(from as usize)
            .map(|(index, entry)| agent_entry(index, entry))
            .collect()
    }

    pub fn state(&self) -> AgentState {
        let replica = self.replica.lock();

        let ended = match (replica.unreachable, self.host.ended(self.link.session())) {
            (true, _) => Some(ViewEnd::Unreachable),
            (false, reason) => reason.map(ViewEnd::from),
        };

        agent_state(replica.view.as_ref(), replica.turn_started, ended)
    }

    /// Take the session back after the person at the host took it.
    pub fn take_control(&self) {
        self.host.reconnect(self.link.session());
    }

    pub async fn submit(&self, text: String) -> Result<SubmitResult, CoreError> {
        let prompt = Prompt {
            fallback_title: fallback_title(&text),
            title_text: text.clone(),
            skill: None,
            images: Vec::new(),
            image_paths: Vec::new(),
            recoverable: Some(RecoverablePrompt {
                text: text.clone(),
                response_annotations: Vec::new(),
                skill: None,
            }),
            text,
        };

        Ok(match self.run(SubmitPrompt(prompt)).await? {
            Ok(submitted) => SubmitResult::Accepted {
                started_turn: submitted.started_turn,
            },
            Err(SubmitRefusal::NotReady) => SubmitResult::NotReady,
            Err(SubmitRefusal::Blocked(SubmissionBlock::QuestionResponse)) => {
                SubmitResult::AnswerPending
            }
            Err(SubmitRefusal::Blocked(SubmissionBlock::ConversationChange)) => {
                SubmitResult::ConversationChanging
            }
            Err(SubmitRefusal::Blocked(SubmissionBlock::CommandStarting)) => {
                SubmitResult::CommandStarting
            }
            Err(SubmitRefusal::Rejected { message }) => SubmitResult::Rejected { message },
        })
    }

    pub async fn interrupt(&self) -> Result<InterruptResult, CoreError> {
        let interruption = self.run(Interrupt).await?;

        Ok(InterruptResult {
            accepted: interruption.outcome == InterruptOutcome::Accepted,
            restored_text: interruption.prompt.map(|(_, prompt)| prompt.text),
        })
    }

    pub async fn respond_approval(
        &self,
        decision: ApprovalDecision,
    ) -> Result<ApprovalResult, CoreError> {
        // The decision names the agent protocols use, which the host passes
        // through to the agent.
        let decision = match decision {
            ApprovalDecision::Accept => "accept",
            ApprovalDecision::AcceptForSession => "acceptForSession",
            ApprovalDecision::Decline => "decline",
            ApprovalDecision::Cancel => "cancel",
        };

        let outcome = self
            .run(RespondApproval {
                decision: decision.to_owned(),
            })
            .await?;

        Ok(match outcome {
            ApprovalOutcome::Ignored => ApprovalResult::Ignored,
            ApprovalOutcome::Rejected => ApprovalResult::Rejected,
            ApprovalOutcome::Waiting => ApprovalResult::Waiting,
            ApprovalOutcome::Settled => ApprovalResult::Settled,
        })
    }

    /// Take a queued message back before the agent reads it. False when the
    /// agent already took it.
    pub async fn withdraw(&self, id: String) -> Result<bool, CoreError> {
        self.run(WithdrawQueuedPrompt { item_id: id }).await
    }

    /// Run the next turns on `model` with `effort`.
    pub async fn select_model(
        &self,
        model: String,
        effort: Option<String>,
    ) -> Result<(), CoreError> {
        let mut settings = self
            .replica
            .lock()
            .view
            .as_ref()
            .map(|view| view.slots.settings.settings.clone())
            .unwrap_or_default();

        settings.model = Some(model);
        settings.effort = effort;

        self.run(ApplyModelSelection { settings }).await?;

        Ok(())
    }

    pub async fn rename(&self, title: String) -> Result<(), CoreError> {
        match self.run(RenameConversation { title }).await? {
            None | Some(Ok(_)) => Ok(()),
            Some(Err(OperationError::Failed(message))) => {
                Err(anyhow!(message).context("renaming failed").into())
            }
            Some(Err(OperationError::Unsupported(_))) => {
                Err(anyhow!("this agent cannot rename its conversations").into())
            }
        }
    }
}

impl Drop for AgentHandle {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Apply the host's updates as they arrive, telling the observer once per
/// burst rather than once per message.
async fn follow(
    mut updates: UnboundedReceiver<AgentUpdate>,
    replica: Arc<Mutex<Replica>>,
    observer: Arc<dyn AgentObserver>,
) {
    while let Some(update) = updates.recv().await {
        let from = {
            let mut replica = replica.lock();
            let mut from = replica.apply(update);

            while let Ok(update) = updates.try_recv() {
                if let Some(at) = replica.apply(update) {
                    from = Some(from.map_or(at, |known| known.min(at)));
                }
            }

            from
        };

        observer.changed(from.map(|from| from as u32));
    }
}

/// The host ending or restoring this device's views changes what the
/// screen shows, though nothing in the view itself changed.
async fn watch_ended(host: Arc<RemoteHost>, observer: Arc<dyn AgentObserver>) {
    let mut changes = host.ended_changes();

    while changes.changed().await.is_ok() {
        observer.changed(None);
    }
}

/// The name a harness without its own title generation gives the
/// conversation: the first line typed, unless it is a slash command.
fn fallback_title(text: &str) -> Option<String> {
    let line = text.lines().find(|line| !line.trim().is_empty())?.trim();

    (!line.starts_with('/')).then(|| line.chars().take(TITLE_CHARS).collect())
}
