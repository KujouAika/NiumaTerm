//! Edits a canvas makes to a definition, with undo and redo.
//!
//! An edit is applied to the definition in memory; validation runs on the
//! result separately, because fixing a graph can pass through invalid
//! states. Undo keeps whole snapshots: a definition has at most a few dozen
//! nodes, so a copy per edit is cheap and needs no inverse per edit kind.

#[cfg(test)]
#[path = "edit_tests.rs"]
mod edit_tests;

use std::mem;

use crate::agent_spec::ProfileReference;
use crate::chat::ThreadSettings;
use crate::orchestration::definition::{Definition, Node, Position, Slot, SlotBody};
use crate::orchestration::graph::{DefinitionError, Graph};
use crate::orchestration::template::Template;

#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Place nodes; coordinates are rounded to whole pixels.
    MoveNodes(Vec<(String, Position)>),
    AddNode {
        id: String,
        slot: String,
        position: Position,
    },
    RemoveNodes(Vec<String>),
    /// Make `to` depend on `from`.
    Connect {
        from: String,
        to: String,
    },
    Disconnect {
        from: String,
        to: String,
    },
    SetNode {
        id: String,
        slot: String,
        prompt: Option<String>,
    },
    RenameNode {
        from: String,
        to: String,
    },
    AddSlot {
        name: String,
        profile: ProfileReference,
    },
    RemoveSlot(String),
    SetSlot {
        name: String,
        body: SlotBody,
    },
    RenameSlot {
        from: String,
        to: String,
    },
    SetMaxParallel(Option<u32>),
}

/// A definition being edited, its undo history, and the version last read
/// from or written to its file.
pub struct Editor {
    definition: Definition,
    history: Vec<Definition>,
    future: Vec<Definition>,
    saved: Definition,
}

impl Editor {
    pub fn open(definition: Definition) -> Self {
        Self {
            saved: definition.clone(),
            definition,
            history: Vec::new(),
            future: Vec::new(),
        }
    }

    pub fn definition(&self) -> &Definition {
        &self.definition
    }

    /// The version last read from or written to the file.
    pub fn saved(&self) -> &Definition {
        &self.saved
    }

    /// Whether the definition differs from the file's version, which
    /// undoing back to that version clears.
    pub fn is_dirty(&self) -> bool {
        self.definition != self.saved
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Every problem the edited definition has, as a file would be checked.
    pub fn errors(&self) -> Vec<DefinitionError> {
        Graph::new(self.definition.clone())
            .err()
            .unwrap_or_default()
    }

    /// Apply `edit`. Returns whether it changed the definition; an edit that
    /// changes nothing leaves the history alone.
    pub fn apply(&mut self, edit: Edit) -> bool {
        let mut next = self.definition.clone();

        if !apply(&mut next, edit) || next == self.definition {
            return false;
        }

        self.history.push(mem::replace(&mut self.definition, next));

        self.future.clear();

        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.history.pop() else {
            return false;
        };

        self.future
            .push(mem::replace(&mut self.definition, previous));

        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.future.pop() else {
            return false;
        };

        self.history.push(mem::replace(&mut self.definition, next));

        true
    }

    /// `written` is now the file's version. A save writes in the background,
    /// so an edit made meanwhile stays unsaved. The history stays, so an
    /// edit made before saving can still be undone.
    pub fn mark_saved(&mut self, written: Definition) {
        self.saved = written;
    }

    /// Replace everything with the file's `definition`, as reloading after an
    /// external edit does.
    pub fn reload(&mut self, definition: Definition) {
        *self = Self::open(definition);
    }

    /// An id no node uses, for a new node.
    pub fn unused_node_id(&self) -> String {
        (1..)
            .map(|number| format!("node-{number}"))
            .find(|id| !self.definition.nodes.iter().any(|node| &node.id == id))
            .unwrap_or_default()
    }

    /// A name no slot uses, for a new slot.
    pub fn unused_slot_name(&self) -> String {
        (1..)
            .map(|number| format!("slot-{number}"))
            .find(|name| !self.definition.slots.iter().any(|slot| &slot.name == name))
            .unwrap_or_default()
    }

    /// The edit that adds a node at `position`: a new id, on the first slot,
    /// or on no slot when the definition has none.
    pub fn new_node(&self, position: Position) -> Edit {
        Edit::AddNode {
            id: self.unused_node_id(),
            slot: self
                .definition
                .slots
                .first()
                .map(|slot| slot.name.clone())
                .unwrap_or_default(),
            position,
        }
    }
}

fn round(position: Position) -> Position {
    Position {
        x: position.x.round(),
        y: position.y.round(),
    }
}

fn node_mut<'a>(definition: &'a mut Definition, id: &str) -> Option<&'a mut Node> {
    definition.nodes.iter_mut().find(|node| node.id == id)
}

/// Apply `edit` to `definition`; `false` when it names something missing or
/// would take a name already in use.
fn apply(definition: &mut Definition, edit: Edit) -> bool {
    match edit {
        Edit::MoveNodes(moves) => {
            for (id, position) in moves {
                if definition.nodes.iter().any(|node| node.id == id) {
                    definition.layout.insert(id, round(position));
                }
            }

            true
        }
        Edit::AddNode { id, slot, position } => {
            if definition.nodes.iter().any(|node| node.id == id) {
                return false;
            }

            definition.layout.insert(id.clone(), round(position));

            definition.nodes.push(Node {
                id,
                slot,
                needs: Vec::new(),
                prompt: None,
            });

            true
        }
        Edit::RemoveNodes(ids) => {
            definition.nodes.retain(|node| !ids.contains(&node.id));

            for node in &mut definition.nodes {
                node.needs.retain(|dependency| !ids.contains(dependency));
            }

            definition.layout.retain(|id, _| !ids.contains(id));

            true
        }
        Edit::Connect { from, to } => {
            if !definition.nodes.iter().any(|node| node.id == from) {
                return false;
            }

            let Some(node) = node_mut(definition, &to) else {
                return false;
            };

            if !node.needs.contains(&from) {
                node.needs.push(from);
            }

            true
        }
        Edit::Disconnect { from, to } => match node_mut(definition, &to) {
            Some(node) => {
                node.needs.retain(|dependency| *dependency != from);

                true
            }
            None => false,
        },
        Edit::SetNode { id, slot, prompt } => match node_mut(definition, &id) {
            Some(node) => {
                node.slot = slot;
                node.prompt = prompt;

                true
            }
            None => false,
        },
        Edit::RenameNode { from, to } => rename_node(definition, &from, to),
        Edit::AddSlot { name, profile } => {
            if definition.slots.iter().any(|slot| slot.name == name) {
                return false;
            }

            definition.slots.push(Slot {
                name,
                body: SlotBody {
                    profile,
                    role: String::new(),
                    settings: ThreadSettings::default(),
                },
            });

            true
        }
        Edit::RemoveSlot(name) => {
            let before = definition.slots.len();

            definition.slots.retain(|slot| slot.name != name);

            definition.slots.len() != before
        }
        Edit::SetSlot { name, body } => {
            match definition.slots.iter_mut().find(|slot| slot.name == name) {
                Some(slot) => {
                    slot.body = body;

                    true
                }
                None => false,
            }
        }
        Edit::RenameSlot { from, to } => {
            if definition.slots.iter().any(|slot| slot.name == to) {
                return false;
            }

            let Some(slot) = definition.slots.iter_mut().find(|slot| slot.name == from) else {
                return false;
            };

            slot.name.clone_from(&to);

            for node in &mut definition.nodes {
                if node.slot == from {
                    node.slot.clone_from(&to);
                }
            }

            true
        }
        Edit::SetMaxParallel(value) => {
            definition.max_parallel = value;

            true
        }
    }
}

/// Rename node `from` to `to` in its own id, every dependency list, the
/// layout, and every template that reads its output. A template that does
/// not parse is left as written; validation already reports it.
fn rename_node(definition: &mut Definition, from: &str, to: String) -> bool {
    if from == to || definition.nodes.iter().any(|node| node.id == to) {
        return false;
    }

    let Some(node) = node_mut(definition, from) else {
        return false;
    };

    node.id.clone_from(&to);

    for node in &mut definition.nodes {
        for dependency in &mut node.needs {
            if dependency == from {
                dependency.clone_from(&to);
            }
        }

        if let Some(prompt) = &mut node.prompt
            && let Ok(mut template) = Template::parse(prompt)
            && template.rename_output(from, &to)
        {
            *prompt = template.to_source();
        }
    }

    if let Some(position) = definition.layout.remove(from) {
        definition.layout.insert(to, position);
    }

    true
}
