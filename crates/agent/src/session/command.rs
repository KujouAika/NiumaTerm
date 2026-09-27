//! What a view asks of a conversation, as values.
//!
//! Every change a view makes to shared conversation state goes through one
//! of these commands, each returning the outcome the view presents. A view
//! beside the session runs them directly; a view in another process sends
//! them to the host, which runs the same code, so the two paths cannot
//! drift apart. Per-view state (drafts in progress, pickers, scroll) stays
//! out of commands.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::chat::{SkillReference, SlashCommandOutcome, SlashCommandRunPolicy, ThreadSettings};
use crate::session::commands::{CommandAdmission, PendingSlashCommand};
use crate::session::controller::{SessionController, SubmissionBlock, UserInterruption};
use crate::session::delivery::RecoverablePrompt;
use crate::session::input::{ApprovalOutcome, QuestionAction, QuestionKey, Submission};
use crate::session::view::DraftAnswers;
use crate::session::{OperationError, SettingsOutcome};

pub trait AgentCommand {
    type Outcome;

    fn run(self, controller: &mut SessionController) -> Self::Outcome;
}

/// One image of a composed message.
#[derive(Clone, Debug)]
pub struct PromptImage {
    pub bytes: Arc<[u8]>,
    pub media_type: String,
}

/// A message as a view composed it, owning its data.
#[derive(Clone, Debug)]
pub struct Prompt {
    /// What the harness receives, response annotations included.
    pub text: String,

    /// What the user typed, which names an unnamed conversation.
    pub title_text: String,

    /// The name a harness without title generation takes from the prompt.
    pub fallback_title: Option<String>,

    pub skill: Option<SkillReference>,
    pub images: Vec<PromptImage>,

    /// The images as files, for a harness that reads images by path.
    pub image_paths: Vec<PathBuf>,

    /// What an interrupt before any answer gives back to the composer.
    pub recoverable: Option<RecoverablePrompt>,
}

/// A message the harness took.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submitted {
    pub started_turn: bool,

    /// The title this prompt claimed for the conversation, shown until the
    /// harness generates one.
    pub title: Option<String>,
}

pub struct SubmitPrompt(pub Prompt);

impl AgentCommand for SubmitPrompt {
    type Outcome = Result<Submitted, SubmitRefusal>;

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        controller.submit_prompt(self.0)
    }
}

/// Why a message was not taken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubmitRefusal {
    Blocked(SubmissionBlock),
    NotReady,
    Rejected { message: String },
}

#[derive(Serialize, Deserialize)]
pub struct Interrupt;

impl AgentCommand for Interrupt {
    type Outcome = UserInterruption;

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        controller.interrupt_from_user()
    }
}

#[derive(Serialize, Deserialize)]
pub struct RespondApproval {
    pub decision: String,
}

impl AgentCommand for RespondApproval {
    type Outcome = ApprovalOutcome;

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        controller.respond_approval(&self.decision)
    }
}

/// Answer or skip a question batch with the answers the view composed.
#[derive(Serialize, Deserialize)]
pub struct AnswerQuestion {
    pub key: QuestionKey,
    pub action: QuestionAction,
    pub answers: DraftAnswers,
}

impl AgentCommand for AnswerQuestion {
    type Outcome = Submission;

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        if let Some(draft) = controller.input_mut().draft_mut(self.key) {
            draft.set_answers(self.answers);
        }

        controller.submit_question(self.key, self.action, Instant::now())
    }
}

/// Replace the thread controls the next turn runs under.
#[derive(Serialize, Deserialize)]
pub struct UpdateSettings {
    pub settings: ThreadSettings,
}

impl AgentCommand for UpdateSettings {
    type Outcome = ();

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        controller.controls.settings = self.settings;
    }
}

/// Adopt `settings` and push its model and effort to a harness that applies
/// them as their own request. `None` means no session is running to ask.
#[derive(Serialize, Deserialize)]
pub struct ApplyModelSelection {
    pub settings: ThreadSettings,
}

impl AgentCommand for ApplyModelSelection {
    type Outcome = Option<SettingsOutcome>;

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        controller.controls.settings = self.settings;

        controller.apply_model_selection()
    }
}

#[derive(Serialize, Deserialize)]
pub struct SelectAgentPreset {
    pub preset: String,
}

impl AgentCommand for SelectAgentPreset {
    type Outcome = Option<SettingsOutcome>;

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        controller.select_agent_preset(self.preset)
    }
}

#[derive(Serialize, Deserialize)]
pub struct WithdrawQueuedPrompt {
    pub item_id: String,
}

impl AgentCommand for WithdrawQueuedPrompt {
    type Outcome = bool;

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        controller.withdraw_queued_prompt(&self.item_id)
    }
}

#[derive(Serialize, Deserialize)]
pub struct RenameConversation {
    pub title: String,
}

impl AgentCommand for RenameConversation {
    type Outcome = Option<Result<String, OperationError>>;

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        controller.rename_conversation(&self.title)
    }
}

#[derive(Serialize, Deserialize)]
pub struct RunSlashCommand {
    pub command: PendingSlashCommand,
}

impl AgentCommand for RunSlashCommand {
    type Outcome = SlashCommandOutcome;

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        controller.execute_command(&self.command)
    }
}

/// Run or queue a slash command typed while a turn is running.
#[derive(Serialize, Deserialize)]
pub struct AdmitSlashCommand {
    pub command: PendingSlashCommand,
    pub policy: SlashCommandRunPolicy,
}

impl AgentCommand for AdmitSlashCommand {
    type Outcome = CommandAdmission;

    fn run(self, controller: &mut SessionController) -> Self::Outcome {
        controller.admit_command_while_busy(self.command, self.policy)
    }
}
