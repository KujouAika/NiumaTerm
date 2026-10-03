## Why

NiumaTerm can run one agent per tab and a moderator-driven Team discussion,
but a user cannot describe a fixed multi-step process such as "plan, then
implement, then review the result against the plan" and have the app run it.
Today that sequence is driven by hand: the user waits for one agent, copies
its reply, and pastes it into the next prompt. A declared graph of agent steps
that NiumaTerm schedules itself removes that manual relay, runs independent
steps in parallel, and keeps a durable record of each run.

## What Changes

- Add **Agent Orchestration**: a run of agent steps declared as a directed
  acyclic graph. The user-facing name is "Orchestration" because the
  `Workflows` name already belongs to the Claude Code Dynamic Workflow view.
- An orchestration definition is a JSON file in the application's own
  directory at `<config>/agent-orchestrations/definitions/<name>.json`, the
  same per-user application folder that holds the logs on Windows and macOS.
  Definitions are personal and available in every workspace; a run uses the
  roots of the workspace it is started in. A definition declares:
  - **agent slots**: a named agent profile reference plus an optional role
    and thread settings. Every node assigned to the same slot runs in the
    same provider conversation.
  - **nodes**: a step bound to one slot, the nodes it depends on, and an
    optional prompt template. A template can reference the run input and the
    output of any ancestor node. A node without a template receives the run
    input or its dependencies' outputs, labeled by node.
- Validation rejects a definition before any agent starts when it has a cycle,
  an unknown slot or dependency, a template reference to a node that is not an
  ancestor, or two nodes on the same slot that the graph does not order.
- NiumaTerm schedules ready nodes, runs nodes on different slots concurrently
  up to a parallelism limit, and stops starting new nodes once a node fails.
- Each run is saved through the generic `SnapshotStore`, with node outputs in
  separate files. The definition is copied into the run when it starts, so
  editing the file never changes a run in progress.
- A run interrupted by closing the app is reopened as interrupted. The user can
  resume a failed, stopped, or interrupted run, which runs every node that has
  not completed again in its slot's resumed conversation.
- A new **Orchestration tab**, opened from the new-tab menu, lists the
  definitions with their validation errors, opens the definitions folder
  for editing, starts a run with an input text, shows each node's state in a
  read-only layered graph, opens a node's turn as a read-only transcript in a
  detail view like the Background Tasks one, and lists recent runs for the
  workspace.
- The feature is hidden behind a new `enable_agent_orchestration` agent
  setting, off by default, appended after the existing agent settings.

Out of scope for this change, planned as later changes: the drag-and-drop
canvas editor, Router, Loop and Join control nodes with skipped-branch
semantics, per-edge context policies other than the final reply text, shell
command nodes, git worktree isolation for parallel nodes, run budgets, and
remote or mobile control of runs.

## Capabilities

### New Capabilities
- `agent-orchestration`: declaring an agent graph in a workspace file,
  validating it, running it with per-slot conversations and parallel
  scheduling, persisting and resuming runs, and the Orchestration tab that
  starts runs and shows their progress.

### Modified Capabilities
<!-- None. The Workflows view, Team rooms and Agent tabs keep their current requirements. -->

## Impact

- `crates/agent`: a new `orchestration` module for the definition model,
  validation, prompt templates, run state, and the pure scheduler; reuses
  `snapshot_store` for run persistence.
- `crates/agent/src/team`: the profile, roots, settings and role fields of
  `Member` move into a shared `AgentSpec` that a slot also uses. Team storage
  must keep decoding existing rooms.
- `crates/app/src/agent_tab`: turn submission moves out of the Team-only
  `MemberHost` into a function that both Team and orchestration slots call; a
  new orchestration runtime and pane host slot sessions and react to their
  `ExecutionSignal`s.
- `crates/app/src/ui`: a new tab surface, a new-tab menu entry, tab restore on
  startup, and a settings toggle.
- `crates/config`: the `enable_agent_orchestration` flag.
- Locale resources: new user-visible strings.
- Data on disk: a new `agent-orchestrations` directory beside `agent-teams`
  in the config directory, holding `definitions/` and `runs/`. Nothing is
  written into workspace roots.
