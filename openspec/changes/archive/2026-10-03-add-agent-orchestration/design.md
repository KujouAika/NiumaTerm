## Context

NiumaTerm already runs agents in three shapes:

- **Agent tabs**: one provider conversation driven by the user.
- **Team rooms** (`nmt_agent::team`, `app::agent_tab::team`): several member
  conversations whose next speaker a moderator or the user decides at run
  time. `TeamSession` holds the durable state and the pure transitions;
  `TeamRuntime` in the app hosts each member's `AgentSession`, submits turns
  through `MemberHost::submit`, and maps the session's `ExecutionSignal`
  (`Accepted { epoch, id }`, `Finished { epoch, id, error, text }`) back to the
  stored attempt.
- **Workflows view**: a read-only display of Claude Code Dynamic Workflow
  runs. The provider decides the graph; NiumaTerm only reads its journal.

None of them lets the user declare a fixed graph of steps that NiumaTerm runs
itself. The generic `SnapshotStore<T>` (commit `84b33bdf`) now provides
locked, revisioned, whole-state persistence that is independent of Team rooms.

The design follows the split between orchestration styles in the OpenAI
Agents SDK: LLM-driven routing (handoffs, agents as tools) maps to Team rooms,
and code-driven flows (chains, parallel fan-out, routing on structured output,
judge loops) map to this feature.

## Goals / Non-Goals

**Goals:**

- A definition format that a person can write and review by hand, and that a
  later canvas editor can write back without reordering semantic fields.
- Validation that catches every structural mistake before an agent starts,
  including template references and slot conflicts.
- A scheduler with no GPUI or I/O dependency, unit-tested in `nmt_agent`.
- Durable runs that never send a prompt twice without the user asking.
- Reuse of the Team session hosting path instead of a second way to drive a
  provider turn.

**Non-Goals:**

- The drag-and-drop canvas, node layout storage, and edge drawing.
- Router, Loop and Join nodes, skipped branches, and run budgets.
- Edge context policies beyond the final reply text (full transcript,
  extracted JSON or diff, summaries).
- Shell nodes, git worktree isolation, remote and mobile control.
- A shared trait over Team and orchestration scheduling. Their scheduling
  models differ (moderator decisions against a static graph), for the same
  reasons the `Backend` trait was rejected.

## Decisions

### D1. Name the feature "Orchestration"

The `Workflows` title-bar control and view already show Claude Code Dynamic
Workflow runs. Reusing the word would make two unrelated views look like one
feature. Alternatives considered: "Pipeline" suggests a linear chain and
misdescribes fan-out; "Flow" collides with the existing workflow wording in
casual use.

### D2. A DAG with slot ordering, no cycles

The top-level graph is acyclic. Repetition (generate, review, retry) arrives
later as a compound Loop node whose body is a sub-graph with a hard iteration
limit, so the outer graph stays acyclic. This keeps topological scheduling,
depth-column display, resume, and later cost estimation simple. Arbitrary
back edges were rejected because a run could then never be shown to end.

Nodes on one slot share a provider conversation, and a conversation accepts
one turn at a time. Validation therefore requires that every pair of nodes on
the same slot is ordered by ancestry. This turns a runtime conflict (a busy
session) into an error the user sees when saving the file. Ancestry is
computed once per definition as a transitive closure over at most 32 nodes, so
the check is a bitset lookup per pair.

### D3. Store definitions and runs in the config directory

Both go under `<config>/agent-orchestrations/`, next to `agent-teams`:
definitions as `definitions/<name>.json`, runs as `runs/<run-id>/`.
`<config>` is `config_dir_path()`. On Windows and macOS it is the same
per-user application folder that holds `logs`; unlike `data_dir()`, which the
log writer uses, it follows `NMT_CONFIG_HOME` and the `--testing` split, so
two test instances never list each other's definitions or runs.

Definitions are personal tools used across projects, so they are not kept in
workspace roots. Alternatives considered: a per-repository
`.niuma/orchestrations` directory would let a team share definitions through
version control, but it writes app files into every repository that uses the
feature, ties a definition to one workspace, and turns a cloned repository
into a source of definitions the user did not write. Sharing can be added
later as an explicit import.

Definitions reference profiles by kind and name only, and validation rejects
credential, endpoint and environment fields. Profiles keep secrets in
protected credential storage; a plain JSON definition holding them would
bypass that storage.

Format sketch:

```json
{
  "version": 1,
  "max_parallel": 3,
  "slots": {
    "dev": { "profile": { "kind": "claude", "name": "Default" },
             "role": "You implement changes in this repository." },
    "critic": { "profile": { "kind": "codex", "name": "Review" } }
  },
  "nodes": [
    { "id": "plan", "slot": "dev",
      "prompt": "Write a short plan for: {{input}}" },
    { "id": "implement", "slot": "dev", "needs": ["plan"],
      "prompt": "Implement the plan above." },
    { "id": "review", "slot": "critic", "needs": ["implement"],
      "prompt": "Review the change against this plan:\n{{plan.output}}" }
  ]
}
```

`slots` is a map and `nodes` is an array so that a hand-written file keeps
the author's node order. A later canvas stores coordinates in a separate
`layout` object so that moving a node never changes the semantic part of the
diff. Decoding uses `deny_unknown_fields`, so a typo such as `need` is an
error and not an ignored field.

### D4. Pure core in `nmt_agent::orchestration`, hosting in the app

`nmt_agent::orchestration` holds:

- `definition`: the decoded types.
- `validate`: structural checks returning every error, not only the first.
- `template`: a parser that turns a prompt into literal and reference
  segments, used both for validation and for rendering.
- `run`: the durable `RunRecord` (definition copy, input, node and slot
  states) and its transitions.
- `schedule`: a pure function from a `RunRecord` plus the set of slots whose
  session is ready to the list of nodes to send next.

`app::agent_tab::orchestration` holds the runtime that owns slot sessions,
persists through a background operation queue like `TeamRuntime`, and the
pane. This mirrors the existing `TeamSession` / `TeamRuntime` split, keeps
the scheduler testable without GPUI, and keeps GPUI types out of
`nmt_agent`.

### D5. Persistence through `SnapshotStore`, outputs in separate files

A run directory contains `run.json` (format `{ file: "run.json", field:
"run", version: 1 }`), `owner.lock`, `prompts/<node>.md`,
`outputs/<node>.md` and `transcripts/<node>.json`. Every commit
rewrites the whole snapshot, so large node outputs would make every state
change cost as much as all outputs together. Outputs are written with
`durable_file::write` first and only then referenced from the snapshot, so a
crash between the two steps leaves an unreferenced file and never a
completed node without its output. The recent-runs list reads snapshots with
the lock-free `snapshot_store::read`, as the Team history list does, so an
open run in another instance neither blocks nor is blocked by the listing.

The run record stores the workspace it was started in, with all of its
roots, so a resumed slot reopens with the same directories; the recent-runs
list filters on the primary root. Every slot uses the roots of that
workspace, so one definition serves every project.

### D6. Node delivery states copied from Team attempts, unified later

Each node goes through `Waiting → Sending → Accepted(turn) → Completed |
Failed`, plus `Stopped`, `Interrupted` and `NotRun`. On reopen, `Sending`
and `Accepted` become `Interrupted` and the run becomes interrupted, as
`TeamSession::open` turns in-flight attempts into `Uncertain`. A node's state
is committed as `Sending` before the prompt is sent, so a crash can never
leave a sent prompt without a record.

Team's `Attempt` is not reused directly: its `DispatchIntent` stores
room-only fields (`PublicSnapshot`, `AcceptedCoverage`, discussion budget
scope), and its transitions update discussion pauses in the same step. Once
this feature works, both will move to a generic `Attempt<I>` whose
transitions return what happened and leave the reaction to the owner; that
refactor needs two real callers to shape its interface and is not part of
this change.

The first version does not try to recover the reply of an interrupted turn
from the resumed provider conversation (Team does this with
`RecoveredTeamTurn`). Resume sends the node again and the tab warns about it.
Matching recovered turns can be added later without a format change, because
the run already stores each node's provider turn id.

### D7. Slot sessions reuse the Team hosting path

Two small extractions come first, as separate refactor commits:

1. `AgentSpec { profile, roots, settings, role }` is moved out of `Member`,
   which embeds it with `#[serde(flatten)]` so that room JSON keeps the same
   keys. A storage test decodes a room snapshot written before the change.
2. The body of `MemberHost::submit` (idle and update-suspension checks,
   title claim, `backend.submit`) becomes a function that takes the prepared
   text, the title source text and the thread settings, and returns
   `SendOutcome`. `MemberHost` and the orchestration slot host both call it.

A slot host owns a `SessionOwner`, subscribes to its `ExecutionSignal`s, and
keys every signal by `(run, node, slot, epoch)`. A signal from an earlier
epoch, or for a turn id other than the one recorded at acceptance, is
ignored, which is the same protection `AttemptEventKey` gives Team. The
slot's provider conversation identity is saved after the session reports it,
and resume starts the session with that `RecoveryIdentity`.

### D8. A minimal template language

Templates support `{{input}}`, `{{<node>.output}}`, and `\{{` for a literal
`{{`. Any other `{{...}}` is a validation error. A general template engine
(minijinja, handlebars) was rejected: conditionals and filters would move
control flow into strings where validation cannot see it, and the static
"references only ancestors" check needs nothing more than the reference list
the small parser returns. Field access other than `.output` is reserved for
later edge context policies.

### D9. Role sent once per slot conversation

A slot's role opens the first text sent in its conversation and is not
repeated. Repeating it every turn would spend context on text the
conversation already contains. A resumed conversation already contains the
role; a conversation that cannot be resumed fails its node, so the case of a
fresh conversation that lost its role does not arise in this version.

### D10. Read-only layered view instead of a canvas

The pane places nodes in columns by dependency depth and lists each node's
dependencies as labels, without drawn edges. This needs only existing layout
primitives and lets the scheduler and persistence be used before the canvas
exists. Session signals arrive outside a frame, so every runtime update that
changes visible state calls `cx.notify`.

### D12. Node details reuse the Background Tasks detail pattern

A node in the graph shows only its identifier, its state and a `View
details` action, so the graph stays readable when outputs are long. The
detail view replaces the graph, as the Background Tasks panel replaces its
list with one child's conversation, and renders through the shared
`TranscriptView`. This reuses the read-only transcript rendering,
virtualization and collapsible work rows that already exist for child agents.

A slot conversation holds the turns of several nodes. Items have no provider
turn id, but every `TranscriptEntry` of a session's `ConversationState`
records the session's local turn number. The runtime saves the slot
session's turn number right after it sends a node, and the detail view of a
running node shows the slot conversation's entries with that turn number
through `TranscriptView::show_attributed_entries`, refreshed as the session
changes. The turn's user message shows the text read from
`prompts/<node>.md`, because a live echo of the user message can omit its
text.

When the node's turn ends, completed, failed or stopped, the items of that
turn are written to `transcripts/<node>.json` before the node's final state
is committed, like its output. A restarted app reads that file when the
detail view opens, so the provider's history never needs to be split by
turn. A node interrupted by the app closing keeps no transcript; resuming
sends it again and the new turn is captured.

Alternative considered: slicing the slot's full provider history at user
messages. It breaks when a node is sent again on resume, when context
compaction rewrites the history, and when a provider omits user text from
its replay.

### D13. Approvals and questions use the slot's own Agent view

A slot agent can stop mid-turn for a tool approval or for answers to its
questions. The runtime reads that from the slot session's input state, the
same check Team uses to block new requests to a waiting member, and the node
shows a needs-input state derived from it; nothing about it is saved,
because a pending request belongs to a live session.

The detail view draws the request with `AgentPane::render_team_interactions`
from one `AgentPane` per slot, created when first needed and kept for the
life of the tab, as `TeamPane` keeps one per member. Binding a second view
to a session would retire the first view's pending answers, so the slot pane
is never recreated while its session is open. Answering there is the same
operation as answering in an Agent tab, so approval scopes, question drafts
and secret inputs behave identically.

Alternatives considered: declining every request automatically, which makes
nodes that need to edit files or run commands fail unless the profile
already allows everything; and refusing to start runs whose slots can ask,
which pushes users toward permissive settings.

### D11. Reload definitions on file change

The pane watches `agent-orchestrations/definitions` with the `notify`
crate, as the
theme settings page watches theme files, and re-reads the definition list
after a short debounce. A definition whose file changes while the user is
starting a run is read once at start and copied into the run (spec: isolate
a run from later definition edits).

## Risks / Trade-offs

- [Resume sends a node again after its side effects already happened, for
  example a half-applied edit] → The tab warns before resuming; the run
  stores each node's turn id so recovered-turn matching can be added.
- [Parallel nodes on different slots write to the same working tree] →
  Documented in the tab's help text; worktree isolation is a planned later
  change. Users can keep write-heavy nodes on one slot, which validation then
  forces into an order.
- [Definitions cannot be shared with a project's other contributors] →
  They are plain files the user can copy; an import action or an opt-in
  per-repository directory can be added once there is demand.
- [Large turns make the node detail view slow] → A node's transcript is
  read from its file only when its details open; the shared transcript
  view virtualizes rows, and its per-child item limit applies.
- [Two instances open the same run] → `SnapshotStore` holds the owner lock;
  the second instance reports the run as open elsewhere.
- [Team storage regression from the `AgentSpec` extraction] → A fixture test
  decodes a room written before the change.

## Migration Plan

No existing data changes format. The feature ships behind
`enable_agent_orchestration`, off by default. Disabling it hides the entry
points and keeps saved runs and definition files. Rolling back the code
leaves `agent-orchestrations` unread.

## Resolved Questions

- A completed run offers no "run again" action; the user starts a new run
  from the definition list.
- A node shows only its state; its turn is read in a detail view built on
  the Background Tasks detail pattern (D12).
