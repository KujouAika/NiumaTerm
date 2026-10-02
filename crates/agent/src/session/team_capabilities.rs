//! Structured moderator operations supported by a live provider session, and
//! what a resumed provider conversation hands back to a Team.

#[derive(Clone, Debug)]
pub struct TeamLaunch {
    pub moderator: bool,
    pub restore_transcript: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeratorAdmission {
    Unavailable,
    CodexDynamicTools { backend_generation: u64 },
}

impl ModeratorAdmission {
    /// Registered operations only admit the backend session that registered
    /// them; a restarted backend must register again before it can moderate.
    pub fn admits(self, backend_generation: u64) -> bool {
        matches!(
            self,
            Self::CodexDynamicTools { backend_generation: registered }
                if registered == backend_generation
        )
    }
}

/// A completed root reply from the resumed provider conversation. Only an
/// exact provider turn identifier can associate it with a saved Team request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredTeamTurn {
    pub id: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use crate::session::team_capabilities::ModeratorAdmission;

    #[test]
    fn moderator_operations_belong_to_the_registered_session() {
        let registered = ModeratorAdmission::CodexDynamicTools {
            backend_generation: 4,
        };

        assert!(registered.admits(4));
        assert!(!registered.admits(5));
        assert!(!ModeratorAdmission::Unavailable.admits(4));
    }
}
