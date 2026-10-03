//! The decoded form of an orchestration definition file.
//!
//! Decoding accepts any structure the JSON can express, including duplicate
//! slot names and out-of-range limits, so that validation can report every
//! problem in a file at once. Unknown fields are refused while decoding: a
//! misspelled key would otherwise be ignored, and a credential or endpoint
//! field would otherwise be accepted into a plain file.

use std::collections::BTreeMap;
use std::fmt::{self, Formatter};

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::agent_spec::ProfileReference;
use crate::chat::ThreadSettings;

/// The only definition format this build reads.
pub const DEFINITION_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub version: u32,

    /// Decoded wide so that a value out of range is a validation error
    /// naming the field, not a decoding error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<u32>,

    #[serde(
        serialize_with = "serialize_slots",
        deserialize_with = "deserialize_slots"
    )]
    pub slots: Vec<Slot>,

    pub nodes: Vec<Node>,

    /// Canvas positions by node id. Kept apart from the nodes so a hand
    /// written node needs no coordinates and moving a node never changes
    /// the node's own lines. Validation, scheduling and prompts ignore it,
    /// and an entry for an id that is not a node is ignored.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub layout: BTreeMap<String, Position>,
}

/// A node's place on the canvas, in logical pixels at 100% zoom.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

/// One agent conversation that nodes are sent to, in file order.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub name: String,
    pub body: SlotBody,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotBody {
    pub profile: ProfileReference,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,

    /// Values set here replace the profile's defaults for this slot.
    #[serde(default)]
    pub settings: ThreadSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub slot: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

fn serialize_slots<S: Serializer>(slots: &[Slot], serializer: S) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(slots.len()))?;

    for slot in slots {
        map.serialize_entry(&slot.name, &slot.body)?;
    }

    map.end()
}

/// Read the `slots` object as pairs. A JSON map type would keep only the
/// last of two entries with the same name, hiding the duplicate.
fn deserialize_slots<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Slot>, D::Error> {
    struct SlotsVisitor;

    impl<'de> Visitor<'de> for SlotsVisitor {
        type Value = Vec<Slot>;

        fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object of named slots")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut slots = Vec::new();

            while let Some((name, body)) = access.next_entry()? {
                slots.push(Slot { name, body });
            }

            Ok(slots)
        }
    }

    deserializer.deserialize_map(SlotsVisitor)
}
