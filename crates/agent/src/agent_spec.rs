//! What an agent conversation starts from, independent of the feature that
//! hosts it: a saved profile, the workspace roots, the thread settings, and
//! the role text that opens the conversation.

use serde::{Deserialize, Serialize};

use crate::AgentWorkspace;
use crate::chat::ThreadSettings;
use crate::session::AgentKind;

/// A lookup into protected profile storage, without resolved launch credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileReference {
    pub kind: AgentKind,
    pub name: String,
}

/// Each holder receives its own copy of settings; profile defaults stay
/// shared only in the profile store, never through mutable spec state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    pub profile: ProfileReference,
    pub roots: AgentWorkspace,
    pub settings: ThreadSettings,
    pub role: String,
}
