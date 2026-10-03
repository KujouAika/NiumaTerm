//! The one format the canvas writes a definition in.
//!
//! Keys come in a fixed order, slots and nodes in definition order, empty
//! fields are left out, and the layout lists nodes in node order with whole
//! numbers. Writing the same definition twice gives the same bytes, so a
//! diff of a saved file shows only what changed. A hand-written file loses
//! its own whitespace and key order on its first save, never its meaning.
//!
//! The format is built from explicit serializers, not a JSON value: a value's
//! object keys would come out in alphabetical order.

#[cfg(test)]
#[path = "canonical_tests.rs"]
mod canonical_tests;

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};

use crate::chat::ThreadSettings;
use crate::orchestration::definition::{Definition, Node, Position, SlotBody};

/// `definition` in the canonical format, ending with a newline.
pub fn to_canonical_json(definition: &Definition) -> String {
    let mut text = serde_json::to_string_pretty(&CanonicalDefinition(definition))
        .expect("a definition always serializes");

    text.push('\n');

    text
}

struct CanonicalDefinition<'a>(&'a Definition);

impl Serialize for CanonicalDefinition<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let definition = self.0;
        let laid_out = Layout(definition);

        let mut map = serializer.serialize_map(None)?;

        map.serialize_entry("version", &definition.version)?;

        if let Some(max_parallel) = definition.max_parallel {
            map.serialize_entry("max_parallel", &max_parallel)?;
        }

        map.serialize_entry("slots", &Slots(definition))?;
        map.serialize_entry("nodes", &Nodes(&definition.nodes))?;

        if laid_out.has_entries() {
            map.serialize_entry("layout", &laid_out)?;
        }

        map.end()
    }
}

struct Slots<'a>(&'a Definition);

impl Serialize for Slots<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.slots.len()))?;

        for slot in &self.0.slots {
            map.serialize_entry(&slot.name, &CanonicalSlot(&slot.body))?;
        }

        map.end()
    }
}

struct CanonicalSlot<'a>(&'a SlotBody);

impl Serialize for CanonicalSlot<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let body = self.0;

        let mut map = serializer.serialize_map(None)?;

        map.serialize_entry("profile", &body.profile)?;

        if !body.role.is_empty() {
            map.serialize_entry("role", &body.role)?;
        }

        if body.settings != ThreadSettings::default() {
            map.serialize_entry("settings", &Settings(&body.settings))?;
        }

        map.end()
    }
}

/// Only the settings a slot sets; an unset one falls back to the profile.
struct Settings<'a>(&'a ThreadSettings);

impl Serialize for Settings<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let settings = self.0;

        let mut map = serializer.serialize_map(None)?;

        for (key, value) in [
            ("model", &settings.model),
            ("approval", &settings.approval),
            ("approvals_reviewer", &settings.approvals_reviewer),
            ("sandbox", &settings.sandbox),
            ("effort", &settings.effort),
            ("tier", &settings.tier),
            ("agent_preset", &settings.agent_preset),
        ] {
            if let Some(value) = value {
                map.serialize_entry(key, value)?;
            }
        }

        map.end()
    }
}

struct Nodes<'a>(&'a [Node]);

impl Serialize for Nodes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;

        // `Node` declares its fields in canonical order and leaves out empty
        // dependency lists and absent prompts.
        for node in self.0 {
            seq.serialize_element(node)?;
        }

        seq.end()
    }
}

/// Positions of the nodes that have one, in node order; entries for ids
/// that are not nodes are dropped.
struct Layout<'a>(&'a Definition);

impl Layout<'_> {
    fn entries(&self) -> impl Iterator<Item = (&str, Position)> {
        self.0.nodes.iter().filter_map(|node| {
            self.0
                .layout
                .get(&node.id)
                .map(|position| (node.id.as_str(), *position))
        })
    }

    fn has_entries(&self) -> bool {
        self.entries().next().is_some()
    }
}

impl Serialize for Layout<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;

        for (id, position) in self.entries() {
            map.serialize_entry(id, &WholePosition(position))?;
        }

        map.end()
    }
}

struct WholePosition(Position);

impl Serialize for WholePosition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;

        map.serialize_entry("x", &(self.0.x.round() as i64))?;
        map.serialize_entry("y", &(self.0.y.round() as i64))?;

        map.end()
    }
}
