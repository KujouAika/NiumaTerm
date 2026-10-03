## Why

Orchestration definitions can only be written by hand as JSON, and a run is
shown as read-only columns of nodes without the dependencies drawn between
them. Building a graph means editing ids and `needs` lists in a text editor
and reading validation errors back from the tab. A canvas that draws the graph
and edits it by dragging makes the structure visible and quick to change,
while hand-written JSON stays a first-class way to author the same files.

## What Changes

- **Canvas view.** The Orchestration tab draws a definition and a run as a
  canvas: node cards placed at stored or computed positions, dependency edges
  drawn as curves between them, panning, and zooming. A run's canvas shows
  each node's state and needs-input mark and keeps the `View details` action,
  replacing the depth columns.
- **Optional `layout` in definitions.** A definition file may hold a
  `layout` object mapping node ids to canvas positions. It is optional: a
  file without it, or a node missing from it, is placed automatically by
  dependency depth. Layout never affects validation, scheduling or prompts.
- **Canvas editing.** For a selected definition the canvas is an editor:
  move nodes, drag from one node's output port to another node to add a
  dependency, select and delete nodes and edges, add a node, and edit the
  selected node (id, slot, prompt template) and the definition's slots
  (profile, role, settings) and the definition's `max_parallel` in
  property panels. Every edit is checked with
  the same validation as a hand-written file, and errors are shown on the
  canvas. Edits can be undone and redone.
- **Same file, both ways.** Saving writes the definition back to its JSON
  file in one canonical format, so a file saved twice is byte-identical and a
  diff shows only what changed. Moving nodes changes only `layout`.
- **External edits.** The canvas tracks unsaved edits. When the file changes
  on disk and the canvas has none, the canvas reloads it; when it has some,
  the tab asks whether to reload the file and discard the canvas edits or keep
  them and overwrite the file on save. A file that does not decode or validate
  opens with its errors and an action to open it in the system's editor, and
  the canvas does not edit it.
- **New definition.** The tab can create an empty definition file from a
  name, which then opens in the canvas.

Delivered in two phases: the canvas view with the run overlay first, then
editing and write-back.

Out of scope: editing a run that already started (runs keep their copy of
the definition), collaborative or multi-window editing of one file,
preserving a hand-written file's own whitespace and key order on save, and
the control nodes (Loop, Router, Join) planned separately.

## Capabilities

### New Capabilities
<!-- None: the canvas extends the existing agent-orchestration capability. -->

### Modified Capabilities
- `agent-orchestration`: a definition may store node positions in an
  optional `layout`; the tab shows definitions and runs on a canvas with
  drawn dependencies instead of depth columns; new requirements cover canvas
  editing, saving in a canonical format, reloading and conflicts on external
  edits, invalid files, undo, and creating a definition.

## Impact

- `crates/agent/src/orchestration`: `Definition` gains an optional `layout`
  field; a canonical writer serializes a definition back to JSON; an edit
  model applies canvas operations to a definition and keeps undo history;
  automatic placement by depth moves from the view into a reusable function.
- `crates/app/src/agent_tab/orchestration`: a canvas element (pan, zoom,
  node cards, bezier edges drawn with GPUI paths, port hit testing, drag
  state), a property panel, editor state with unsaved tracking and conflict
  prompts, and the file watcher wired to reload decisions.
- Locale resources: new strings for editing actions, the conflict prompt
  and the property panel.
- Compatibility: files without `layout` keep working unchanged. A file saved
  with `layout` is rejected by builds from before this change, because
  definitions refuse unknown fields; the feature is still behind its
  off-by-default setting.
