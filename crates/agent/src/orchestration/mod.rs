//! Agent orchestration: a user-declared acyclic graph of agent steps that
//! NiumaTerm runs itself.
//!
//! Each node is one turn sent to a named slot, and every node on a slot runs
//! in that slot's single provider conversation. This module holds the
//! definition model, its validation, prompt templates and composition; it
//! has no GPUI or file system dependency so every rule is unit-testable.

pub mod compose;
pub mod definition;
pub mod graph;
pub mod template;
