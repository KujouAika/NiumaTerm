## 1. Phase 1: layout model and placement (`nmt_agent::orchestration`)

- [x] 1.1 Add `Position` and the optional `layout` map to `Definition` (default empty, omitted when empty); keep `Graph::new` independent of it; test that files without `layout` and files with entries for unknown ids both decode and validate
- [x] 1.2 Add `placement::place` returning one position per node: the stored one, else depth column and row, with cycle nodes at depth 0 and computed positions kept off stored ones; unit-test stored, computed, mixed and cyclic definitions

## 2. Phase 1: canvas view (`app::agent_tab::orchestration`)

- [x] 2.1 Add the canvas entity with viewport state (offset, zoom clamped to 25%-200%) and a pure interaction state machine for panning, with unit tests for its transitions and the viewport transform
- [x] 2.2 Render node cards at transformed positions and paint dependency edges as cubic beziers from output to input ports with GPUI paths; hide the state line below 50% zoom
- [x] 2.3 Pan by dragging the background and by scrolling, zoom around the pointer with Ctrl+scroll, add a fit-to-view action, open every canvas fitted; call `cx.notify` from every pointer handler that changes state
- [x] 2.4 Replace the depth columns of a run with the read-only canvas placed from the run's definition copy, keeping each node's state, needs-input mark and `View details`
- [x] 2.5 Show a selected definition on a read-only canvas; show decode failures with their error and an action that opens the file in the system editor
- [x] 2.6 Add a view test that draws a run canvas and asserts every node card and the edge layer are laid out inside the canvas bounds

## 3. Phase 2: edit model (`nmt_agent::orchestration::edit`)

- [x] 3.1 Define `Edit` and `Editor` with apply, undo and redo by definition snapshot and a saved-state comparison for the dirty mark
- [x] 3.2 Implement node edits: move (positions rounded to integers), add with an unused id and the first slot, remove (also from dependencies and layout), connect (appended, duplicates ignored), disconnect, set slot and prompt
- [x] 3.3 Implement renames: a node across dependencies, layout and template references through the template parser, leaving escaped text alone; a slot across node assignments
- [x] 3.4 Implement slot edits (add, remove, set profile, role and settings) and SetMaxParallel (1 to 8, or none for the default)
- [x] 3.5 Implement the canonical writer and test byte stability, key order, layout in node order, the final newline, and that a hand-written compact file keeps its meaning after one save
- [x] 3.6 Unit-test every edit, rename, undo and redo, and the dirty mark returning to clean after undoing to the saved state

## 4. Phase 2: editing on the canvas

- [x] 4.1 Extend the interaction state machine with node dragging (after a small movement threshold), connecting from an output port, and selection of nodes and edges with bezier hit testing; unit-test the transitions
- [x] 4.2 Apply canvas gestures as edits: one MoveNodes per drag, Connect on drop over a node, Delete removing the selection, an add-node action; Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y for undo and redo
- [x] 4.3 Validate after every edit, list the errors and mark the nodes and slots they name
- [x] 4.4 Add the node property panel (id, slot choice, prompt template with insertable ancestor references) the slots panel (name, profile choice from saved profiles, role, settings, add and remove), and the definition settings panel for `max_parallel`
- [x] 4.5 Show the unsaved mark and save with Ctrl+S or the save action through `durable_file::write` on the background executor, remembering the written bytes

## 5. Phase 2: files, conflicts and prompts

- [x] 5.1 Route watcher events for the open file through the editor: ignore its own bytes, reload when clean keeping the viewport, show the Reload / Keep mine banner when dirty, report deletion and recreate on save, switch to the error view when the file stops decoding
- [x] 5.2 Ask to save, discard or cancel before switching definition or run with unsaved edits
- [x] 5.3 Add the unsaved-edits condition to the shell's tab-close and window-close confirmations, with Save continuing the close after writing
- [x] 5.4 Add New definition: name validation, refusing an existing name, writing the version-only file, opening it on the canvas
- [x] 5.5 Add all new strings to the English and Chinese catalogs

## 6. Verification

- [x] 6.1 In a testing instance, open a hand-written definition without `layout`, pan, zoom around the pointer and fit; confirm edges match its dependencies and a run of it shows states on the canvas
- [x] 6.2 With real input, drag nodes, connect two nodes, delete an edge and a node, rename a node referenced by a template, undo and redo, and save; confirm the file diff contains only the expected changes and a second save changes nothing
- [x] 6.3 Edit the open file in a text editor with and without unsaved canvas edits; confirm the silent reload, the conflict banner and both of its choices, and that the canvas's own save triggers neither
- [x] 6.4 Close the tab and quit the app with unsaved edits; confirm the prompt and that Cancel keeps the edits
