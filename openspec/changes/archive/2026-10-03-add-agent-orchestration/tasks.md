## 1. Shared extractions (separate refactor commits)

- [x] 1.1 Add a Team storage test that decodes a room snapshot fixture written by the current code, so the next step can prove the on-disk keys are unchanged
- [x] 1.2 Move `profile`, `roots`, `settings` and `role` from `Member` into a new `AgentSpec` embedded with `#[serde(flatten)]`; update `MemberConfig`, room validation and the app's Team views; the fixture test from 1.1 passes unchanged
- [x] 1.3 Extract the body of `MemberHost::submit` into a function taking prepared text, title source text and `ThreadSettings` and returning `SendOutcome`; `MemberHost::submit` calls it with the same behavior

## 2. Definition model and validation (`nmt_agent::orchestration`)

- [x] 2.1 Add the `orchestration` module with `definition` types (slots map, ordered nodes, `needs`, optional prompt, `max_parallel` default 3, range 1 to 8) decoded with `deny_unknown_fields`
- [x] 2.2 Implement the template parser for `{{input}}`, `{{<node>.output}}` and `\{{`, returning literal and reference segments and an error for any other `{{...}}`
- [x] 2.3 Implement `validate` returning every error with its file, node, slot or field: empty graph, more than 32 nodes, empty slot name, duplicate ids, unknown slot or dependency, repeated dependency, empty prompt, cycles, malformed or non-ancestor template references, unordered nodes on one slot, credential, endpoint or environment fields
- [x] 2.4 Unit-test each validation rule, including the accepted case of ordered nodes sharing a slot
- [x] 2.5 Implement prompt composition (template rendering, no-template root, no-template node with headed dependency outputs in listed order, role prefix for a slot's first sent node) and the "definition needs an input" check, with unit tests

## 3. Run state and scheduling

- [x] 3.1 Define `RunRecord` (definition copy, input, workspace root, run state, node states with failure reasons and provider turn ids, slot conversation identities and sent flags) and its serde form
- [x] 3.2 Implement the pure node transitions: waiting, sending, accepted, completed, failed, stopped, not run, interrupted; reject transitions from the wrong state
- [x] 3.3 Implement `schedule(record, ready_slots)` returning the nodes to send next, honoring dependencies, `max_parallel`, slot readiness, and the rule that no node starts after a failure or stop
- [x] 3.4 Implement run completion, failure after running siblings finish, stop, reopen (in-flight nodes and the run become interrupted) and resume (completed nodes kept, the rest return to waiting)
- [x] 3.5 Unit-test scheduling and lifecycle: parallel siblings, parallelism limit, failure with a running sibling, stop, reopen, resume after failure
- [x] 3.6 Store the run's full `AgentWorkspace` instead of its primary root, filtering recent runs on the primary root

## 4. Persistence

- [x] 4.1 Add `RunStore` wrapping `SnapshotStore<RunRecord>` under `<config>/agent-orchestrations/runs/<run-id>/` with format `run.json` / `run` / version 1
- [x] 4.2 Write the sent text to `prompts/<node>.md` before the node is recorded as sending, and node outputs to `outputs/<node>.md` with `durable_file::write` before committing the node as completed; read them on demand
- [x] 4.3 Add a lock-free recent-runs listing filtered by workspace root, using `snapshot_store::read`, tolerant of damaged runs
- [x] 4.4 Test restart with outputs, the owner lock against a second open, and an output file written without its commit being ignored
- [x] 4.5 Add saving and loading of a node's transcript items as `transcripts/<node>.json`, written with `durable_file::write`, with a restart test

## 5. App runtime

- [x] 5.1 Add `enable_agent_orchestration` to the agent config, after the existing flags, with default-off and round-trip tests
- [x] 5.2 Add the orchestration runtime: a background operation queue over `RunStore`, slot hosts built on `SessionOwner` and the submit function from 1.3, sessions started lazily per slot
- [x] 5.3 Map `ExecutionSignal`s to node transitions keyed by run, node, slot and epoch; ignore stale epochs and mismatched turn ids; save slot conversation identities when reported
- [x] 5.4 Implement start (profile lookup with a missing-profile error, definition copy), stop (interrupt running turns), resume (warn, then start slot sessions with their `RecoveryIdentity`), and failure when a conversation cannot be resumed
- [x] 5.5 Call `cx.notify` for every runtime change that alters visible state, since session signals arrive outside a frame
- [x] 5.6 Save the slot session's local turn number when a node is sent; when its turn ends, save the entries with that turn number, with the user message text replaced by the saved sent text, to `transcripts/<node>.json` before committing the node's final state; load that file for ended nodes
- [x] 5.7 Derive each running node's needs-input state from its slot session's input state, and keep one `AgentPane` per slot, created on first use and kept while the session is open, for drawing its requests

## 6. Orchestration tab

- [x] 6.1 Add the tab surface, the `New orchestration` entry at the end of the new-tab menu when the setting is on, and tab restore of the shown run on startup
- [x] 6.2 List definitions from `<config>/agent-orchestrations/definitions/*.json` with validation errors and the expected directory when missing; add the action that creates and opens that directory in the system file manager; watch it with `notify` and reload after a debounce
- [x] 6.3 Add the run input, the start action (disabled for invalid definitions and for an empty required input), and the recent-runs list with definition name, state and start time
- [x] 6.4 Show the selected run as depth columns where each node shows only its id, its state and a `View details` action; add stop and resume actions with the resume warning
- [x] 6.5 Add the node detail view following the Background Tasks detail: it replaces the graph, shows a header with id, slot, state and elapsed time, attaches the node's `ConversationState` to `TranscriptView`, shows the failure reason or "not run", extends live while the node runs, and has a back action
- [x] 6.6 Add the settings toggle and all new strings to the locale resources
- [x] 6.7 Show the needs-input state on the node and in the run view, and draw the slot pane's pending approval or questions below the node's transcript in its detail view

## 7. Verification

- [x] 7.1 Launch `NiumaTerm.exe --testing` with the setting on and run a three-node definition (plan and implement on one slot, review on another) to completion; confirm outputs, the role sent once, and the review prompt containing the plan output
- [x] 7.2 Run a definition with two parallel slots, stop it mid-run, then resume it; confirm completed nodes are not sent again
- [x] 7.3 Exit the app during a running node, relaunch, and confirm the run shows as interrupted and sends nothing until resumed
- [x] 7.4 Edit a definition file while its run is running and confirm the run is unchanged and the list shows the new validation result
- [x] 7.5 Launch a second instance with `--testing` and its own `NMT_CONFIG_HOME` and confirm it lists neither the first instance's definitions nor its runs
