# Agent Orchestration Specification

## Purpose

Run a user-declared acyclic graph of agent steps: definitions saved in the application directory, validated before any agent starts, scheduled across per-slot agent conversations with parallel nodes, persisted durably so runs can be stopped, reopened and resumed, and shown in an Orchestration tab whose nodes open their own turn and answer their agent's requests.

## Requirements

### Requirement: Gate orchestration behind an agent setting
The system SHALL provide an `enable_agent_orchestration` agent setting that is off by default and is listed after the existing agent settings. While it is off, the new-tab menu SHALL NOT offer an Orchestration entry, and saved Orchestration tabs SHALL NOT be restored on startup. Turning it off SHALL NOT delete saved runs or definition files.

#### Scenario: Setting is off
- **WHEN** `enable_agent_orchestration` is off and the user opens the new-tab menu
- **THEN** the menu offers no Orchestration entry

#### Scenario: Setting is turned on
- **WHEN** the user turns `enable_agent_orchestration` on in the agent settings
- **THEN** the new-tab menu offers a `New orchestration` entry after the existing entries

#### Scenario: Setting is turned off with saved runs
- **WHEN** the user turns the setting off and later turns it on again
- **THEN** previously saved runs are listed again in the Orchestration tab

### Requirement: Discover definitions in the application directory
The system SHALL read orchestration definitions from the files matching `agent-orchestrations/definitions/*.json` in the configuration directory, the same directory that holds the Team rooms. Each file SHALL be one definition, named by its file stem. The same definitions SHALL be listed in every workspace, and the system SHALL NOT read or write definition files in workspace roots. The Orchestration tab SHALL list every such file, including files that fail to decode or validate, and SHALL offer an action that opens the definitions directory in the system file manager, creating it when missing.

#### Scenario: Definitions exist
- **WHEN** the definitions directory contains `review.json`
- **THEN** the Orchestration tab lists a definition named `review` in any workspace

#### Scenario: No definitions directory
- **WHEN** the definitions directory does not exist
- **THEN** the tab reports that no definitions exist, names the expected directory, and offers to open it

#### Scenario: Open the definitions directory
- **WHEN** the user chooses to open the definitions directory and it does not exist
- **THEN** the system creates it and opens it in the system file manager

#### Scenario: Testing instance
- **WHEN** an instance runs with `--testing` or with its own `NMT_CONFIG_HOME`
- **THEN** it lists only the definitions in its own configuration directory

#### Scenario: A file is edited while the tab is open
- **WHEN** the user saves a change to a listed definition file and no run of it is starting
- **THEN** the tab shows the definition decoded from the saved file, with its new validation result

### Requirement: Declare agent slots and nodes
A definition SHALL declare a set of agent slots and a set of nodes. A slot SHALL have a unique name, a reference to a saved agent profile by kind and name, an optional role text, and optional thread settings that override the profile's defaults. A node SHALL have a unique identifier made of lowercase ASCII letters, digits, `_` and `-`, the name of one slot, a list of identifiers of the nodes it depends on, and an optional prompt template. A definition SHALL accept an optional `max_parallel` between 1 and 8, which SHALL default to 3. A definition SHALL accept an optional `layout` object that maps node identifiers to canvas positions, each with an `x` and a `y` coordinate. The layout SHALL NOT affect validation, scheduling, or prompts; a layout entry for an identifier that is not a node SHALL be ignored, and a node without an entry SHALL be placed automatically. A definition SHALL NOT store credentials, endpoints, or environment values; those SHALL come from the referenced profile.

#### Scenario: Minimal two-step definition
- **WHEN** a definition declares slot `dev` and nodes `plan` and `implement`, where `implement` depends on `plan`
- **THEN** the definition is valid and `implement` runs only after `plan` completes

#### Scenario: Credentials in a definition
- **WHEN** a definition file contains an API key or environment field for a slot
- **THEN** validation rejects the definition and names the field

#### Scenario: Layout is optional
- **WHEN** a hand-written definition has no `layout`
- **THEN** it is valid and its nodes are placed on the canvas by dependency depth

#### Scenario: Layout entry for a removed node
- **WHEN** the `layout` names `draft`, which is not a node of the definition
- **THEN** the definition is valid and the entry is ignored

### Requirement: Validate a definition before any agent starts
The system SHALL reject a definition, and SHALL NOT start any agent for it, when any of the following holds: it declares no node; it declares more than 32 nodes; a slot has an empty name; two nodes or two slots share an identifier; a node names an unknown slot or depends on an unknown node; a node lists the same dependency more than once; a node declares a prompt that is empty or contains only whitespace; the dependencies form a cycle; a prompt template contains a malformed reference or references a node that is not an ancestor of the node; or two nodes on the same slot are not ordered by the dependency graph, meaning neither is an ancestor of the other. Each validation error SHALL name the file and the node, slot, or field it concerns. A referenced profile that does not exist SHALL be reported when the user tries to start a run.

#### Scenario: Cycle
- **WHEN** node `a` depends on `b` and `b` depends on `a`
- **THEN** validation rejects the definition and names both nodes

#### Scenario: Template references a non-ancestor
- **WHEN** node `review` has the template `{{test.output}}` and `test` is not an ancestor of `review`
- **THEN** validation rejects the definition and names `review` and `test`

#### Scenario: Unordered nodes share a slot
- **WHEN** nodes `frontend` and `backend` both use slot `dev` and neither depends on the other directly or transitively
- **THEN** validation rejects the definition and names both nodes and the slot

#### Scenario: Ordered nodes share a slot
- **WHEN** nodes `plan` and `implement` both use slot `dev` and `implement` depends on `plan`
- **THEN** validation accepts the definition

#### Scenario: Dependency listed twice
- **WHEN** node `merge` lists `frontend` twice in its dependencies
- **THEN** validation rejects the definition and names `merge` and `frontend`

#### Scenario: Empty prompt
- **WHEN** node `plan` declares a prompt containing only whitespace
- **THEN** validation rejects the definition, names `plan`, and explains that removing the prompt sends the node its inputs

#### Scenario: Slot with an empty name
- **WHEN** a definition declares a slot whose name is the empty string
- **THEN** validation rejects the definition and reports the empty slot name

#### Scenario: Missing profile
- **WHEN** a valid definition references a profile that is not saved and the user starts a run
- **THEN** the run does not start and the tab names the slot and the missing profile

### Requirement: Compose each node's prompt
The system SHALL build the text sent for a node as follows. When the node has a template, every `{{input}}` SHALL be replaced by the run input and every `{{<node>.output}}` SHALL be replaced by that node's output; a backslash immediately before `{{` SHALL produce a literal `{{`. When the node has no template and no dependencies, the text SHALL be the run input. When the node has no template and has dependencies, the text SHALL be each dependency's output in the order the node lists them, each preceded by a heading naming that dependency. When this is the first node sent in its slot's conversation and the slot has a role, the role SHALL precede the composed text. A node's output SHALL be the final reply text of the turn the node started.

#### Scenario: Template with input and an ancestor output
- **WHEN** node `review` has the template `Check this against the plan:\n{{plan.output}}\nTask: {{input}}`
- **THEN** the text sent contains the output of `plan` and the run input in those positions

#### Scenario: No template with two dependencies
- **WHEN** node `merge` has no template and depends on `frontend` and `backend`
- **THEN** the text sent contains the output of `frontend` and then the output of `backend`, each under a heading naming its node

#### Scenario: Role is sent once per slot conversation
- **WHEN** slot `dev` has a role and nodes `plan` and `implement` run in it in that order
- **THEN** the text sent for `plan` starts with the role and the text sent for `implement` does not

#### Scenario: Empty run input
- **WHEN** the run input is empty and some node's composed text would include the run input
- **THEN** the tab does not allow the run to start and reports that the definition needs an input

### Requirement: Schedule nodes
The system SHALL start a node only after every node it depends on has completed. It SHALL run ready nodes on different slots concurrently, with at most `max_parallel` nodes running at once. Every node assigned to a slot SHALL run in that slot's single provider conversation, which the system SHALL start when the slot's first node becomes ready. A run SHALL complete when every node has completed.

#### Scenario: Independent nodes run in parallel
- **WHEN** nodes `frontend` and `backend` depend only on `plan`, use different slots, and `max_parallel` is 3
- **THEN** both start after `plan` completes, without waiting for each other

#### Scenario: Parallelism limit
- **WHEN** four ready nodes on different slots exist and `max_parallel` is 2
- **THEN** two nodes run and the others start as running nodes complete

#### Scenario: A slot continues its conversation
- **WHEN** `implement` runs after `plan` in slot `dev`
- **THEN** `implement` is sent as a new turn in the same conversation that answered `plan`

### Requirement: Stop starting work after a failure
When a node fails, meaning its turn ends with an error, its slot's session cannot start, or its turn is rejected, the system SHALL mark the node failed with the reason, SHALL start no further node in the run, SHALL let nodes already running finish and keep their outputs, and SHALL then mark the run failed. Nodes that never started SHALL be shown as not run.

#### Scenario: Failure with a sibling running
- **WHEN** `frontend` fails while `backend` is running
- **THEN** `backend` runs to its end, no other node starts, and the run ends failed with `frontend` named as the cause

### Requirement: Stop a run on request
The system SHALL let the user stop a running run. Stopping SHALL interrupt every running node's turn, SHALL start no further node, and SHALL mark the run stopped. Interrupted nodes SHALL be shown as stopped and nodes that never started as not run.

#### Scenario: User stops a run
- **WHEN** the user stops a run while `implement` is running
- **THEN** the `implement` turn is interrupted, no further node starts, and the run is shown as stopped

### Requirement: Isolate a run from later definition edits
The system SHALL copy the decoded definition into the run when the run starts and SHALL schedule and display the run from that copy. Editing, renaming, or deleting the definition file SHALL NOT change a run that already started, including when it is resumed.

#### Scenario: Definition edited during a run
- **WHEN** the user adds a node to `review.json` while a run of `review` is running
- **THEN** that run neither starts nor shows the added node

### Requirement: Persist runs durably
The system SHALL save each run under `agent-orchestrations/runs` in the configuration directory, one directory per run. The saved state SHALL include the definition copy, the run input, the text sent for each node that was sent, each node's state and failure reason, each slot's provider conversation identity, each completed node's output, and the transcript items of every node turn that ended. A node's output SHALL be saved before the node is recorded as completed. A run SHALL be owned by one NiumaTerm instance at a time; another instance SHALL be told that the run is open elsewhere.

#### Scenario: Outputs survive a restart
- **WHEN** a run completes and the app restarts
- **THEN** reopening the run shows every node's output

#### Scenario: Run opened by another instance
- **WHEN** one instance has a run open and a second instance tries to open it
- **THEN** the second instance reports that the run is open elsewhere and does not modify it

### Requirement: Reopen an interrupted run
When the system reopens a run that was saved as running, it SHALL mark that run interrupted and every node that was running as interrupted, and SHALL NOT send any prompt until the user resumes the run.

#### Scenario: App closes during a run
- **WHEN** the app exits while `implement` is running and the user later reopens the run
- **THEN** the run is shown as interrupted, `implement` is shown as interrupted, and no prompt is sent

### Requirement: Resume a run
The system SHALL let the user resume a failed, stopped, or interrupted run. Resuming SHALL keep every completed node and its output, SHALL return every other node to waiting, SHALL resume each slot's saved provider conversation before sending its next node, and SHALL then schedule the run as a new attempt of those nodes. Before resuming, the tab SHALL warn that nodes which were running will be sent again and that work they already did is not undone. When a slot's saved conversation cannot be resumed, the node waiting on it SHALL fail with that reason.

#### Scenario: Resume after a failure
- **WHEN** `test` failed after `plan` and `implement` completed, and the user resumes the run
- **THEN** `plan` and `implement` are not sent again and `test` is sent in its slot's resumed conversation

#### Scenario: Conversation cannot be resumed
- **WHEN** the user resumes a run and the provider cannot resume slot `dev`'s saved conversation
- **THEN** the next node on `dev` fails with the reason and no other slot's work is affected until the failure stops the run

### Requirement: Show runs in an Orchestration tab
The system SHALL provide an Orchestration tab, opened from the new-tab menu for the active workspace. The tab SHALL list the definitions and their validation errors, SHALL let the user enter a run input and start a run of a valid definition, and SHALL list recent runs of the workspace with their definition name, state, and start time. For a selected run it SHALL show the run's nodes on a read-only canvas, placed by the layout of the definition copy the run started with, with every dependency drawn as an edge from the dependency to the node that needs it. The run canvas SHALL support panning and zooming. Each node SHALL show only its identifier, its run state, and a `View details` action; it SHALL NOT show its prompt, output, or transcript inline. An open Orchestration tab SHALL be restored on startup with the run it showed.

#### Scenario: Start a run
- **WHEN** the user selects the valid definition `review`, enters an input, and starts it
- **THEN** a new run appears in the recent runs list and its nodes are shown with their states

#### Scenario: Node shows its state
- **WHEN** a run is shown with `plan` completed and `implement` running
- **THEN** `plan` and `implement` each show their identifier, their state, and a `View details` action, and no prompt or output text

#### Scenario: Dependencies are drawn
- **WHEN** a run of a definition where `review` needs `frontend` and `backend` is shown
- **THEN** the canvas draws an edge from `frontend` to `review` and an edge from `backend` to `review`

#### Scenario: Run keeps its layout
- **WHEN** nodes of definition `review` are moved and saved after a run of it started
- **THEN** that run's canvas still places its nodes where the definition placed them when the run started

#### Scenario: Invalid definition
- **WHEN** the user selects a definition that failed validation
- **THEN** the tab shows its errors and does not offer to start it

#### Scenario: Restore on startup
- **WHEN** the app restarts with an Orchestration tab open on a run
- **THEN** the tab is restored showing that run

### Requirement: Show a node's turn in a detail view
Choosing `View details` on a node SHALL replace the run's graph with a detail view of that node, in the same way the Background Tasks view replaces its list with one task's conversation. The detail view SHALL show a header with the node's identifier, slot, state, and elapsed time, followed by a read-only transcript of the node's turn: the text sent for it, the agent's reasoning, tool activity, file changes and command output, and its final reply, rendered by the same transcript view the Background Tasks detail uses. A failed node SHALL show its failure reason. A node that never started SHALL show that it has not run. While the node runs, its transcript SHALL extend as the agent works. The detail view SHALL offer a back action that returns to the graph. The transcript of a node turn that ended SHALL remain viewable after the app restarts.

#### Scenario: Open a completed node
- **WHEN** the user chooses `View details` on the completed node `plan`
- **THEN** the graph is replaced by `plan`'s header and the transcript of its turn, ending with its final reply

#### Scenario: Watch a running node
- **WHEN** the user opens the details of `implement` while it runs
- **THEN** new reasoning, tool activity and replies appear in the transcript as the agent produces them

#### Scenario: Only the node's own turn is shown
- **WHEN** `plan` and `implement` ran in the same slot conversation and the user opens the details of `implement`
- **THEN** the transcript shows the turn sent for `implement` and none of `plan`'s turn

#### Scenario: Return to the graph
- **WHEN** the user chooses the back action in a node's detail view
- **THEN** the run's graph is shown again

#### Scenario: Details after a restart
- **WHEN** the app restarts and the user opens the details of a node that completed before the restart
- **THEN** the transcript of its turn is shown as it was when the node ended

### Requirement: Answer an agent's request from node details
While a node's agent waits for the user, for a tool approval or for answers to its questions, the node SHALL show a state that it needs input, and the Orchestration tab SHALL make that visible without opening the node. The node's detail view SHALL show the pending approval or questions below the transcript with the same controls an Agent tab offers for them, and answering there SHALL continue the node's turn exactly as answering in an Agent tab would. Nodes on other slots SHALL keep running while one node waits. Stopping the run SHALL interrupt a waiting node like any running node.

#### Scenario: Approval requested
- **WHEN** the agent running `implement` asks for approval to edit a file
- **THEN** `implement` shows that it needs input, and its detail view shows the approval with approve and decline controls

#### Scenario: Answer continues the turn
- **WHEN** the user approves the request in `implement`'s detail view
- **THEN** the agent continues the same turn and `implement` shows that it is running again

#### Scenario: Questions are answered in details
- **WHEN** the agent running `plan` asks the user two questions
- **THEN** `plan`'s detail view shows both questions with the controls an Agent tab uses, and submitting the answers continues the turn

#### Scenario: Other slots keep running
- **WHEN** `frontend` waits for an approval while `backend` runs on another slot
- **THEN** `backend` runs to its end and its dependents start as usual

### Requirement: Navigate the canvas
The canvas SHALL pan when the user drags its empty background or scrolls, SHALL zoom around the pointer when the user scrolls with Ctrl held, and SHALL keep its zoom between 25% and 200%. It SHALL offer an action that fits every node into view. A canvas SHALL open fitted to its nodes.

#### Scenario: Zoom around the pointer
- **WHEN** the user holds Ctrl and scrolls up with the pointer over node `plan`
- **THEN** the canvas zooms in and `plan` stays under the pointer

#### Scenario: Fit to view
- **WHEN** nodes lie outside the visible area and the user chooses the fit action
- **THEN** every node is visible

### Requirement: Edit a definition on the canvas
Selecting a definition that decodes SHALL open it on an editable canvas, whether or not it passes validation. The user SHALL be able to move nodes by dragging them, add a dependency by dragging from a node's output port onto another node, select a node or an edge and delete it, and add a node. A new node SHALL receive an unused identifier and the first slot of the definition, or no slot when there is none. Deleting a node SHALL remove it from every other node's dependencies and from the layout. A dependency SHALL be added at the end of the node's dependency list, and adding one the node already has SHALL do nothing. After every edit the canvas SHALL validate the edited definition as a file would be validated, list the errors, and mark the nodes and slots they name; edits that make the definition invalid SHALL be allowed, because fixing a graph can pass through invalid states.

#### Scenario: Connect two nodes
- **WHEN** the user drags from `plan`'s output port onto `implement`
- **THEN** `implement` depends on `plan` and an edge from `plan` to `implement` is drawn

#### Scenario: Connection that forms a cycle
- **WHEN** the user connects `implement` to `plan` while `implement` already depends on `plan`
- **THEN** the dependency is added, the cycle error naming both nodes is listed, and both nodes are marked

#### Scenario: Delete a node
- **WHEN** the user selects `draft`, which `review` depends on, and deletes it
- **THEN** `draft` and its edges disappear and `review` no longer lists it

#### Scenario: Moving a node changes only the layout
- **WHEN** the user drags `plan` to a new position and saves
- **THEN** the saved file differs only in `plan`'s `layout` entry

### Requirement: Edit node and slot properties
The canvas SHALL show a property panel for the selected node with its identifier, its slot chosen from the definition's slots, and its prompt template, a panel for the definition's slots where each slot's name, profile chosen from the saved profiles, role, and thread settings can be edited and slots can be added and removed, and a definition settings panel where `max_parallel` can be set to a value from 1 to 8 or cleared to use the default of 3. Renaming a node SHALL rename it in every dependency list, in the layout, and in every prompt template that references its output. Renaming a slot SHALL rename it in every node assigned to it. Property edits SHALL be validated like canvas edits.

#### Scenario: Rename a node
- **WHEN** the user renames `plan` to `outline` while `review`'s template contains `{{plan.output}}`
- **THEN** `review` depends on `outline` and its template contains `{{outline.output}}`

#### Scenario: Insert an ancestor's output
- **WHEN** the user edits `review`'s prompt in the panel
- **THEN** the panel offers the outputs of `review`'s ancestors as references to insert

#### Scenario: Set the parallelism limit
- **WHEN** the user sets `max_parallel` to 2 in the definition settings panel and saves
- **THEN** the file stores `max_parallel` as 2, and clearing the value removes the key from the file on the next save

#### Scenario: Remove a slot in use
- **WHEN** the user removes slot `web` while node `frontend` uses it
- **THEN** the slot is removed and the error that `frontend` uses an unknown slot is listed

### Requirement: Undo canvas edits
The canvas SHALL keep a history of the edits made since the definition was opened or last reloaded, and SHALL let the user undo and redo them with Ctrl+Z and Ctrl+Shift+Z or Ctrl+Y. Moving a node by one drag SHALL be one history entry. Saving SHALL NOT clear the history.

#### Scenario: Undo a deletion
- **WHEN** the user deletes `draft` and then presses Ctrl+Z
- **THEN** `draft`, its position, and its edges are restored

### Requirement: Save a definition in a canonical format
The canvas SHALL show when it holds unsaved edits and SHALL save them to the definition's file with Ctrl+S or a save action, also when the definition is invalid. The file SHALL be written in one canonical format: indented JSON with keys in a fixed order (`version`, `max_parallel` when set, `slots`, `nodes`, `layout` when any node has a position), slots and nodes in their definition order, each node's keys in a fixed order, layout entries in node order with whole-number coordinates, and a final newline. Saving an unchanged definition SHALL produce identical bytes. Saving a hand-written file SHALL replace its whitespace and key order with the canonical format and SHALL NOT change its slots, nodes, dependencies, prompts, or settings.

#### Scenario: Save twice
- **WHEN** the user saves a definition and saves it again without editing
- **THEN** the file's bytes are unchanged by the second save

#### Scenario: Hand-written file keeps its meaning
- **WHEN** a hand-written definition with compact one-line nodes is opened and saved without edits
- **THEN** the reformatted file decodes to the same slots, nodes, dependencies, prompts, and settings

### Requirement: Reconcile external edits to an open definition
The system SHALL detect when the file of the definition open on the canvas changes on disk through anything other than the canvas's own save. When the canvas holds no unsaved edits it SHALL reload the file, keeping the current pan and zoom, and SHALL briefly show that it reloaded the file. When it holds unsaved edits it SHALL keep them and ask the user either to reload the file and discard the canvas edits, or to keep the canvas edits, which the next save writes over the file. When the file is deleted the canvas SHALL keep its edits, report the deletion, and recreate the file on save. When the reloaded file no longer decodes, the canvas SHALL show its errors as for any undecodable file.

#### Scenario: External edit without unsaved changes
- **WHEN** the user edits `review.json` in a text editor while the canvas shows it with no unsaved edits
- **THEN** the canvas shows the edited definition and a short notice that it was reloaded from disk

#### Scenario: External edit with unsaved changes
- **WHEN** the canvas has an unsaved move of `plan` and `review.json` changes on disk
- **THEN** the move is kept and the tab asks whether to reload the file or keep the canvas edits

#### Scenario: Keep canvas edits
- **WHEN** the user chooses to keep the canvas edits and then saves
- **THEN** the file holds the canvas's version

#### Scenario: Own save is not an external edit
- **WHEN** the canvas saves `review.json`
- **THEN** no reload or conflict prompt follows

### Requirement: Open undecodable definitions as text
A definition file that does not decode SHALL be shown with its decoding error and an action that opens the file in the system's default editor, and SHALL NOT open on the canvas. Once the file is fixed on disk the canvas SHALL open it.

#### Scenario: JSON syntax error
- **WHEN** the user selects `broken.json`, which has a missing comma
- **THEN** the tab shows the error with its position and an action to open the file, and no canvas

### Requirement: Protect unsaved canvas edits
When the canvas holds unsaved edits, the system SHALL ask whether to save, discard, or cancel before selecting another definition, before closing the Orchestration tab, its workspace, or its window, and before quitting the app. Showing a run SHALL keep the edits without asking, mark the definition as unsaved in the list, and show them again when the definition is selected.

#### Scenario: Switch definitions with unsaved edits
- **WHEN** the canvas has unsaved edits and the user selects another definition
- **THEN** the tab asks to save, discard, or cancel, and cancel keeps the current definition and its edits

#### Scenario: View a run with unsaved edits
- **WHEN** the canvas has unsaved edits and the user opens a recent run, then selects the definition again
- **THEN** no prompt appears, the definition is listed as unsaved meanwhile, and the canvas shows the edits again

#### Scenario: Quit with unsaved edits
- **WHEN** the user quits the app while a canvas has unsaved edits
- **THEN** the app asks to save, discard, or cancel before quitting

### Requirement: Create a definition from the tab
The tab SHALL offer an action that creates a definition from a name made of letters, digits, `_` and `-`. It SHALL refuse a name already used by a definition file. The new file SHALL contain only the version, no slots, and no nodes, and SHALL open on the canvas, where it is listed as invalid until it declares a node.

#### Scenario: New definition
- **WHEN** the user creates a definition named `triage`
- **THEN** `triage.json` is written to the definitions directory and opens on an empty canvas

#### Scenario: Name in use
- **WHEN** the user creates a definition named `review` and `review.json` exists
- **THEN** no file is written and the tab reports that the name is in use
