## MODIFIED Requirements

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

## ADDED Requirements

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
