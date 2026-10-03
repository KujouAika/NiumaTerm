//! Which nodes of a run to send next.

use crate::orchestration::graph::Graph;
use crate::orchestration::run::{NodeState, RunRecord, RunState};

/// What the runtime should do after a change to the run or its sessions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dispatch {
    /// Nodes to compose and send now, in graph order.
    pub send: Vec<usize>,

    /// Slots whose next node is ready but whose session is not, in the order
    /// their nodes were found. Starting a session that is already starting
    /// is the caller's to skip.
    pub prepare: Vec<usize>,
}

/// `ready[slot]` is whether that slot's session is started and idle.
///
/// A node is ready when it is waiting and every node it depends on has
/// completed. Ready nodes are taken in graph order until the run has
/// `max_parallel` nodes running; a node whose slot is not ready still takes
/// a place, so no more sessions start than the run can use.
pub fn schedule(graph: &Graph, run: &RunRecord, ready: &[bool]) -> Dispatch {
    let mut dispatch = Dispatch::default();

    if run.state() != RunState::Running {
        return dispatch;
    }

    let nodes = run.nodes();

    let running = nodes
        .iter()
        .filter(|node| node.state.is_in_flight())
        .count();

    let mut capacity = graph.max_parallel().saturating_sub(running);

    for &node in graph.order() {
        if capacity == 0 {
            break;
        }

        let waiting = nodes[node].state == NodeState::Waiting;

        let unblocked = graph
            .needs(node)
            .iter()
            .all(|&dependency| nodes[dependency].state.is_completed());

        if !(waiting && unblocked) {
            continue;
        }

        let slot = graph.slot_of(node);

        if ready.get(slot).copied().unwrap_or(false) {
            dispatch.send.push(node);
        } else if !dispatch.prepare.contains(&slot) {
            dispatch.prepare.push(slot);
        }

        capacity -= 1;
    }

    dispatch
}
