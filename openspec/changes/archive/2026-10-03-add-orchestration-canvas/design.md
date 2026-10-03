## Context

Agent Orchestration v1 (archived as `2026-10-03-add-agent-orchestration`)
defines graphs in hand-written JSON files under
`<config>/agent-orchestrations/definitions` and shows runs as read-only
columns by dependency depth. Definitions are decoded with
`deny_unknown_fields` into `Definition`, validated by `Graph::new`, and copied
into each `RunRecord` at start. The tab watches the definitions directory
with `notify` and reloads the list on change.

GPUI offers the drawing primitives a canvas needs (`canvas()` for custom
paint, `PathBuilder::stroke` with `cubic_bezier_to`, `Window::paint_path`) but
no node-editor component, and nothing in the app uses those path APIs yet.
GPUI's frame pump wakes only on `cx.notify` or `window.refresh`, so every
mouse-driven change must notify. Its test harness cannot model real drags,
and synthetic pointer input from PowerShell does not always pass GPUI's
hover checks, so drag interactions need verification with real input.

The user decided that the canvas and hand-written JSON both stay: the file
remains the definition, the canvas is an editor of that file, and external
edits are reconciled with unsaved canvas edits by reloading when there are
none and asking when there are some.

## Goals / Non-Goals

**Goals:**

- One canvas component for both a run (read-only, with states) and a
  definition (editable), so the two never look different.
- All edit logic in `nmt_agent`, unit-tested without GPUI: applying edits,
  renames across references, undo, canonical writing, and placement.
- Saving an unchanged definition is byte-stable, and moving nodes touches
  only `layout`.
- No silent loss: unsaved edits are never discarded without a choice.

**Non-Goals:**

- Preserving a hand-written file's own formatting on save.
- Editing a run in progress.
- Multi-select, copy and paste, auto-layout beyond depth placement, minimap.
- Loop, Router and Join nodes.

## Decisions

### D1. `layout` is a separate optional object keyed by node id

`Definition` gains `layout: BTreeMap<String, Position>` (`#[serde(default,
skip_serializing_if = "BTreeMap::is_empty")]`), with `Position { x: f32, y:
f32 }`. Keeping positions out of the node objects means a hand-written node
never needs coordinates and a move never changes a node's lines. Unknown
ids are ignored on load (spec) and dropped on save, so deleting a node
cannot leave stale entries behind. `Graph::new` does not read the layout.
Positions are in canvas units (logical pixels at 100% zoom).

Alternative: `x`/`y` on each node. Rejected because every move would edit
the node's own object and every hand-written node would gain two keys.

### D2. Canonical writer instead of format-preserving edits

Saving serializes the in-memory `Definition` with a dedicated writer: two
space indentation, keys in a fixed order, layout in node order with
coordinates rounded to integers, final newline. Hand-written formatting is
replaced on the first save.

Alternative: edit the original text in place with a JSON syntax tree,
keeping the author's whitespace and order. Rejected for this change: it
needs a lossless JSON parser and edit mapping for every operation, for a
benefit that ends after the first save of a file anyway. The proposal lists
it as out of scope.

`serde_json`'s map serialization follows field declaration order for
structs, so the writer is `serde_json::to_writer_pretty` over the existing
types with the field order already canonical, plus the slots map written in
`Vec<Slot>` order (as now) and the layout written in node order through a
small `Serialize` adapter. Rounding happens when a drag ends, so the model
and the file agree.

### D3. Edits are a value type applied to a `Definition`, with undo by snapshot

`nmt_agent::orchestration::edit` defines `Edit` (MoveNodes, AddNode,
RemoveNodes, Connect, Disconnect, SetNode { id, slot, prompt }, RenameNode,
AddSlot, RemoveSlot, SetSlot, RenameSlot, SetMaxParallel) and `Editor {
definition, history,
future, saved }`. Applying an edit clones the definition onto the history
stack before changing it; undo and redo swap definitions between stacks.
Definitions are at most 32 nodes, so whole-value snapshots cost little and
avoid writing an inverse for each edit kind. A drag records one MoveNodes
when the pointer is released, not one per frame. `saved` holds the
definition last read from or written to disk; the canvas is dirty when the
current definition differs from it, so undoing back to the saved state
clears the dirty mark without extra bookkeeping.

Renames rewrite dependency lists, layout keys and template references; the
template rewrite goes through the existing parser so only real
`{{id.output}}` references change and escaped `\{{` text is left alone.

### D4. Validation reuses `Graph::new` and maps errors to canvas elements

After each edit the editor runs `Graph::new` on a clone and keeps the
errors. Each `DefinitionError` variant already names its node, slot or
field, so the canvas marks those elements and lists the messages. A
decodable but invalid definition opens and saves normally (spec), so a user
can fix a graph through invalid intermediate states and never ends up with a
file the canvas cannot reopen. Only decode failures (syntax, unknown or
mistyped fields) keep the file off the canvas.

### D5. Placement: stored position, else depth columns

`nmt_agent::orchestration::placement::place(definition) -> Vec<Position>`
returns the stored position when the layout has one, else a computed one:
column by dependency depth, row by order within the column, the same grid
the v1 tab used. For a graph that does not validate (a cycle has no depth),
depth falls back to 0 for the nodes in the cycle so the canvas still shows
them. Nodes without stored positions are placed in free grid cells so they
do not cover stored ones. The run canvas places from the run's definition
copy, which includes its layout.

### D6. Canvas element: absolute node views plus one painted edge layer

The canvas is a GPUI entity holding viewport state (`offset`, `zoom`) and an
interaction state machine (Idle, Panning, DraggingNodes, Connecting { from,
pointer }). It renders:

- one `canvas()` element filling the area that paints edges as cubic
  beziers from a node's output port (right edge) to the dependent node's
  input port (left edge), plus the in-progress connection line;
- node cards as absolutely positioned divs at `offset + position * zoom`,
  with sizes and text scaled by zoom.

Cards stay plain GPUI elements so text layout, focus and the existing
button components keep working; only edges are custom-painted. Hit testing
for edges (selection, deletion) uses distance from the pointer to the
sampled bezier, within a few pixels at the current zoom. Mouse handlers sit
on the canvas root and translate positions with the viewport transform;
each handler that changes state calls `cx.notify`, since pointer events
arrive outside a frame. A card drag starts only after the pointer moves a
few pixels, so a click on `View details` or a port is not read as a move.

Alternative: draw cards with the painter too. Rejected: it would reimplement
text layout and buttons.

### D7. Editor state, file identity and conflict handling in the view layer

`DefinitionEditor` (app side) owns the `Editor`, the file path, and the
bytes last written or read. The definitions watcher already fires on every
change; for the open file the editor re-reads it and compares with those
bytes, so its own save never counts as an external edit (spec: "Own save is
not an external edit"). Then:

- bytes equal: nothing to do;
- not dirty: decode and reload, keeping `offset` and `zoom`;
- dirty: keep the edits and show a banner with `Reload` (replace and clear
  history) and `Keep mine` (dismiss; the next save overwrites);
- file missing: banner reporting the deletion; save recreates the file.

Saving writes with `durable_file::write` on the background executor and
records the written bytes before the watcher can report them.

### D8. Unsaved-edit prompts join the existing close confirmations

Switching to another definition inside the tab asks through a banner in the
pane. Showing a run does not ask: the editor stays alive behind the run
view, the definition list marks it unsaved, and selecting the definition
again shows the edits, so nothing can be lost there. Closing the tab, its
workspace or the window routes through the shell's existing confirmation
paths (`request_close_tab`, `request_close_workspace`,
`request_window_close`), which first ask about any Orchestration surface
whose pane reports unsaved edits. Save in that dialog saves and then repeats
the close, so the close's own confirmations still follow; Discard drops the
edits and repeats it; Cancel aborts it.

### D9. Phasing

Phase 1 ships the canvas view: placement, the read-only canvas for runs
(replacing the depth columns) and for selected definitions, pan, zoom and
fit. Phase 2 adds `layout` decoding (already needed for display in phase 1
when present), editing, the property panels, undo, saving, external-edit
handling, unsaved-edit prompts and New definition. Phase 1 reads `layout`
but never writes it, so its files stay byte-identical.

## Risks / Trade-offs

- [Synthetic drags fail GPUI hover checks, and unit tests cannot drag] →
  The interaction state machine is a plain struct whose transitions are
  unit-tested; end-to-end drags are verified with real input in a testing
  instance.
- [Files saved with `layout` are rejected by builds older than this change]
  → The feature is behind an off-by-default setting; noted in the proposal.
- [Edge hit testing over curves is approximate] → Sampling the bezier at
  fixed steps and using a zoom-scaled tolerance is enough for at most 32
  nodes; selection also works from the property panel.
- [The watcher reports partial writes from editors that save in steps] →
  A file that fails to decode while the canvas is clean switches to the
  error view and back on the next successful write; a dirty canvas keeps its
  edits and only shows the conflict banner.
- [Large zoom changes make text unreadable] → Zoom is clamped to 25%–200%;
  below 50% cards hide their state line and show only the id.

## Migration Plan

No data migration. Phase 1 changes only how definitions and runs are
displayed. Phase 2 adds an optional field that existing files do not have.

## Resolved Questions

- `max_parallel` is editable in a definition settings panel (1 to 8, or
  cleared to fall back to the default of 3), through a SetMaxParallel edit.
