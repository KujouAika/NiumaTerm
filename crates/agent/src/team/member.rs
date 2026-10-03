use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::AgentWorkspace;
use crate::agent_spec::{AgentSpec, ProfileReference};
use crate::chat::ThreadSettings;
use crate::team::model::{MemberId, MessageId, SummaryId};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedCoverage {
    pub messages: BTreeSet<MessageId>,
    pub summaries: BTreeSet<SummaryId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub(super) id: MemberId,
    pub(super) name: String,

    /// Flattened so saved rooms keep the member keys they had before the
    /// spec became a shared type.
    #[serde(flatten)]
    pub(super) spec: AgentSpec,

    pub(super) excluded: bool,
    #[serde(default)]
    pub(super) provider_id: Option<String>,
    #[serde(default)]
    pub(super) moderator_registered: bool,
}

pub struct MemberConfig {
    pub name: String,
    pub spec: AgentSpec,
}

impl Member {
    pub fn provider_id(&self) -> Option<&str> {
        self.provider_id.as_deref()
    }

    pub fn moderator_registered(&self) -> bool {
        self.moderator_registered
    }

    pub fn id(&self) -> MemberId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn profile(&self) -> &ProfileReference {
        &self.spec.profile
    }

    pub fn roots(&self) -> &AgentWorkspace {
        &self.spec.roots
    }

    pub fn settings(&self) -> &ThreadSettings {
        &self.spec.settings
    }

    pub fn role(&self) -> &str {
        &self.spec.role
    }

    pub fn excluded(&self) -> bool {
        self.excluded
    }
}
