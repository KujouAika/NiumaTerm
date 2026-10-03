//! The durable state of one orchestration run and its transitions.
//!
//! Every transition checks the state it starts from and returns whether it
//! applied, so a late or repeated provider signal cannot move a node twice.
//! Signals are matched by the session epoch the node was sent under and by
//! the provider turn id it was accepted as, because a restarted session can
//! reuse turn ids and an earlier session can still deliver events.

#[cfg(test)]
#[path = "run_tests.rs"]
mod run_tests;

use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::{Error as UuidError, Uuid};

use crate::AgentWorkspace;
use crate::orchestration::definition::Definition;
use crate::orchestration::graph::Graph;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl FromStr for RunId {
    type Err = UuidError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

impl Display for RunId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    /// A node failed while others were still running. No node starts; the
    /// run fails once the running ones finish, keeping their outputs.
    Failing,
    Completed,
    Failed,
    Stopped,
    /// The app closed while the run was going. Nothing is sent until the
    /// user resumes it.
    Interrupted,
}

impl RunState {
    pub fn is_resumable(self) -> bool {
        matches!(self, Self::Failed | Self::Stopped | Self::Interrupted)
    }

    fn is_live(self) -> bool {
        matches!(self, Self::Running | Self::Failing)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NodeState {
    #[default]
    Waiting,
    /// Committed before the prompt is sent, so a crash can never leave a
    /// sent prompt without a record.
    Sending {
        epoch: u64,
    },
    Accepted {
        epoch: u64,
        turn: String,
    },
    /// The output is saved in its own file before this state is committed.
    Completed {
        turn: String,
    },
    Failed {
        reason: String,
    },
    /// The user stopped the run while this node was running.
    Stopped,
    /// The app closed while this node was running.
    Interrupted,
    /// The run ended before this node started.
    NotRun,
}

impl NodeState {
    pub fn is_in_flight(&self) -> bool {
        matches!(self, Self::Sending { .. } | Self::Accepted { .. })
    }

    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRun {
    pub state: NodeState,

    /// Milliseconds since the Unix epoch.
    pub started_at: Option<u64>,

    pub finished_at: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotRun {
    /// The provider's conversation id once the session reports it; resume
    /// reopens this conversation.
    pub conversation: Option<String>,

    /// Whether the provider accepted a turn in this conversation, which
    /// delivered the slot's role; later turns are sent without it.
    pub opened: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    id: RunId,
    definition_name: String,

    /// A copy taken at start, so edits to the definition file never change
    /// a run that already started.
    definition: Definition,

    input: String,

    /// The workspace the run was started in, with all of its roots, so a
    /// slot resumed later opens with the same directories.
    workspace: AgentWorkspace,

    started_at: u64,
    ended_at: Option<u64>,
    state: RunState,
    nodes: Vec<NodeRun>,
    slots: Vec<SlotRun>,
}

impl RunRecord {
    pub fn new(
        definition_name: String,
        graph: &Graph,
        input: String,
        workspace: AgentWorkspace,
        now: u64,
    ) -> Self {
        Self {
            id: RunId::new(),
            definition_name,
            definition: graph.definition().clone(),
            input,
            workspace,
            started_at: now,
            ended_at: None,
            state: RunState::Running,
            nodes: vec![NodeRun::default(); graph.node_count()],
            slots: vec![SlotRun::default(); graph.slot_count()],
        }
    }

    pub fn id(&self) -> RunId {
        self.id
    }

    pub fn definition_name(&self) -> &str {
        &self.definition_name
    }

    pub fn definition(&self) -> &Definition {
        &self.definition
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn workspace(&self) -> &AgentWorkspace {
        &self.workspace
    }

    pub fn started_at(&self) -> u64 {
        self.started_at
    }

    pub fn ended_at(&self) -> Option<u64> {
        self.ended_at
    }

    pub fn state(&self) -> RunState {
        self.state
    }

    pub fn nodes(&self) -> &[NodeRun] {
        &self.nodes
    }

    pub fn slots(&self) -> &[SlotRun] {
        &self.slots
    }

    /// Mark `node` as being sent under session `epoch`.
    pub fn begin_send(&mut self, node: usize, epoch: u64, now: u64) -> bool {
        if self.state != RunState::Running || self.nodes[node].state != NodeState::Waiting {
            return false;
        }

        self.nodes[node] = NodeRun {
            state: NodeState::Sending { epoch },
            started_at: Some(now),
            finished_at: None,
        };

        true
    }

    /// The provider accepted the turn sent for `node` as `turn`.
    pub fn accept(&mut self, graph: &Graph, node: usize, epoch: u64, turn: &str) -> bool {
        if !matches!(self.nodes[node].state, NodeState::Sending { epoch: sent } if sent == epoch) {
            return false;
        }

        self.nodes[node].state = NodeState::Accepted {
            epoch,
            turn: turn.to_owned(),
        };

        self.slots[graph.slot_of(node)].opened = true;

        true
    }

    /// The turn `node` was accepted as finished. The caller has already
    /// saved its output.
    pub fn complete(&mut self, node: usize, epoch: u64, turn: &str, now: u64) -> bool {
        let accepted = matches!(
            &self.nodes[node].state,
            NodeState::Accepted { epoch: sent, turn: accepted } if *sent == epoch && accepted == turn
        );

        if !accepted {
            return false;
        }

        self.nodes[node].state = NodeState::Completed {
            turn: turn.to_owned(),
        };

        self.nodes[node].finished_at = Some(now);

        self.refresh_state(now);

        true
    }

    /// `node` failed. `epoch` is the session it was sent under, or `None`
    /// when its slot's session could not start or resume before sending.
    pub fn fail(&mut self, node: usize, epoch: Option<u64>, reason: String, now: u64) -> bool {
        let applies = match (&self.nodes[node].state, epoch) {
            (NodeState::Waiting, None) => true,
            (
                NodeState::Sending { epoch: sent } | NodeState::Accepted { epoch: sent, .. },
                Some(epoch),
            ) => *sent == epoch,
            _ => false,
        };

        if !self.state.is_live() || !applies {
            return false;
        }

        self.nodes[node].state = NodeState::Failed { reason };
        self.nodes[node].finished_at = Some(now);

        self.refresh_state(now);

        true
    }

    /// Stop the run on the user's request. Returns the nodes whose turns
    /// were running, which the caller interrupts.
    pub fn stop(&mut self, now: u64) -> Vec<usize> {
        if !self.state.is_live() {
            return Vec::new();
        }

        let mut running = Vec::new();

        for (index, node) in self.nodes.iter_mut().enumerate() {
            if node.state.is_in_flight() {
                node.state = NodeState::Stopped;
                node.finished_at = Some(now);

                running.push(index);
            } else if node.state == NodeState::Waiting {
                node.state = NodeState::NotRun;
            }
        }

        self.state = RunState::Stopped;
        self.ended_at = Some(now);

        running
    }

    /// Record a run reopened after the app closed while it was going. The
    /// turns it was waiting for belonged to sessions that no longer exist.
    pub fn interrupt(&mut self) -> bool {
        if !self.state.is_live() {
            return false;
        }

        for node in &mut self.nodes {
            if node.state.is_in_flight() {
                node.state = NodeState::Interrupted;
            }
        }

        self.state = RunState::Interrupted;

        true
    }

    /// Return every node that has not completed to waiting. Completed nodes
    /// and their outputs are kept and never sent again.
    pub fn resume(&mut self) -> bool {
        if !self.state.is_resumable() {
            return false;
        }

        for node in &mut self.nodes {
            if !node.state.is_completed() {
                *node = NodeRun::default();
            }
        }

        self.state = RunState::Running;
        self.ended_at = None;

        true
    }

    /// Save the provider conversation id `slot`'s session reported.
    pub fn record_conversation(&mut self, slot: usize, conversation: String) -> bool {
        if self.slots[slot].conversation.as_deref() == Some(conversation.as_str()) {
            return false;
        }

        self.slots[slot].conversation = Some(conversation);

        true
    }

    /// Recompute the run state after a node finished.
    fn refresh_state(&mut self, now: u64) {
        let failed = self
            .nodes
            .iter()
            .any(|node| matches!(node.state, NodeState::Failed { .. }));

        let in_flight = self.nodes.iter().any(|node| node.state.is_in_flight());

        if failed && in_flight {
            self.state = RunState::Failing;
        } else if failed {
            for node in &mut self.nodes {
                if node.state == NodeState::Waiting {
                    node.state = NodeState::NotRun;
                }
            }

            self.state = RunState::Failed;
            self.ended_at = Some(now);
        } else if self.nodes.iter().all(|node| node.state.is_completed()) {
            self.state = RunState::Completed;
            self.ended_at = Some(now);
        }
    }
}
