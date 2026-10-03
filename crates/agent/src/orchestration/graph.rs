//! Validation of a decoded definition into a [`Graph`] that is safe to run.
//!
//! Validation collects every error instead of stopping at the first, so a
//! user editing a definition by hand sees all of its problems at once.
//! Checks that need the dependency order (template ancestry, slot ordering)
//! run only when the dependencies themselves are sound, because their
//! answers are meaningless on a graph with a cycle or a missing node.

#[cfg(test)]
#[path = "graph_tests.rs"]
mod graph_tests;

use std::collections::BTreeMap;

use thiserror::Error;

use crate::orchestration::definition::{DEFINITION_VERSION, Definition};
use crate::orchestration::template::{Template, TemplateError, is_node_id};

/// Ancestor sets are `u32` bitmasks indexed by node position.
pub const MAX_NODES: usize = 32;

pub const DEFAULT_MAX_PARALLEL: usize = 3;

const MAX_PARALLEL_RANGE: (u32, u32) = (1, 8);

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DefinitionError {
    #[error("version {0} is not supported; this build reads version 1")]
    UnsupportedVersion(u32),
    #[error("the definition declares no node")]
    NoNodes,
    #[error("the definition declares {0} nodes; at most 32 are allowed")]
    TooManyNodes(usize),
    #[error("`max_parallel` is {0}; it must be between 1 and 8")]
    MaxParallelOutOfRange(u32),
    #[error("a slot has an empty name")]
    EmptySlotName,
    #[error("slot `{0}` is declared more than once")]
    DuplicateSlot(String),
    #[error("node id `{0}` must use only lowercase letters, digits, `_` and `-`")]
    InvalidNodeId(String),
    #[error("node `{0}` is declared more than once")]
    DuplicateNode(String),
    #[error("node `{node}` uses unknown slot `{slot}`")]
    UnknownSlot { node: String, slot: String },
    #[error("node `{node}` depends on unknown node `{dependency}`")]
    UnknownDependency { node: String, dependency: String },
    #[error("node `{node}` lists dependency `{dependency}` more than once")]
    RepeatedDependency { node: String, dependency: String },
    #[error("nodes {} depend on each other in a cycle", quoted(.0))]
    Cycle(Vec<String>),
    #[error("node `{node}` has an empty prompt; remove `prompt` to send its inputs")]
    EmptyPrompt { node: String },
    #[error("node `{node}` prompt: {error}")]
    Template { node: String, error: TemplateError },
    #[error("node `{node}` reads `{reference}`, which is not one of its ancestors")]
    NotAncestor { node: String, reference: String },
    #[error("nodes `{first}` and `{second}` share slot `{slot}` but neither depends on the other")]
    UnorderedSlot {
        slot: String,
        first: String,
        second: String,
    },
}

fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A definition that passed validation, with the relations the scheduler
/// and the prompt composer read precomputed. Node and slot indices follow
/// the order of the definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    definition: Definition,
    slot_of: Vec<usize>,
    needs: Vec<Vec<usize>>,
    ancestors: Vec<u32>,
    templates: Vec<Option<Template>>,
    order: Vec<usize>,
    depth: Vec<usize>,
}

impl Graph {
    pub fn new(definition: Definition) -> Result<Self, Vec<DefinitionError>> {
        let mut errors = Vec::new();

        if definition.version != DEFINITION_VERSION {
            errors.push(DefinitionError::UnsupportedVersion(definition.version));
        }

        if let Some(value) = definition.max_parallel
            && !(MAX_PARALLEL_RANGE.0..=MAX_PARALLEL_RANGE.1).contains(&value)
        {
            errors.push(DefinitionError::MaxParallelOutOfRange(value));
        }

        let slots = index_slots(&definition, &mut errors);
        let nodes = index_nodes(&definition, &mut errors);

        let mut slot_of = Vec::with_capacity(definition.nodes.len());
        let mut needs = Vec::with_capacity(definition.nodes.len());
        let mut templates = Vec::with_capacity(definition.nodes.len());
        let mut resolved = true;

        for node in &definition.nodes {
            match slots.get(node.slot.as_str()) {
                Some(&slot) => slot_of.push(slot),
                None => {
                    errors.push(DefinitionError::UnknownSlot {
                        node: node.id.clone(),
                        slot: node.slot.clone(),
                    });

                    slot_of.push(usize::MAX);
                }
            }

            let mut indices: Vec<usize> = Vec::with_capacity(node.needs.len());

            for dependency in &node.needs {
                match nodes.get(dependency.as_str()) {
                    Some(&index) if indices.contains(&index) => {
                        errors.push(DefinitionError::RepeatedDependency {
                            node: node.id.clone(),
                            dependency: dependency.clone(),
                        });
                    }
                    Some(&index) => indices.push(index),
                    None => {
                        errors.push(DefinitionError::UnknownDependency {
                            node: node.id.clone(),
                            dependency: dependency.clone(),
                        });

                        resolved = false;
                    }
                }
            }

            needs.push(indices);

            templates.push(match node.prompt.as_deref() {
                None => None,
                Some(prompt) if prompt.trim().is_empty() => {
                    errors.push(DefinitionError::EmptyPrompt {
                        node: node.id.clone(),
                    });

                    None
                }
                Some(prompt) => match Template::parse(prompt) {
                    Ok(template) => Some(template),
                    Err(error) => {
                        errors.push(DefinitionError::Template {
                            node: node.id.clone(),
                            error,
                        });

                        None
                    }
                },
            });
        }

        // Duplicate node ids make every index-based relation ambiguous, and
        // an oversized graph does not fit the ancestor bitmask.
        let unique = nodes.len() == definition.nodes.len();
        let fits = definition.nodes.len() <= MAX_NODES;

        if !(resolved && unique && fits) {
            return Err(errors);
        }

        let order = match topological_order(&needs) {
            Ok(order) => order,
            Err(cycle) => {
                errors.push(DefinitionError::Cycle(
                    cycle
                        .into_iter()
                        .map(|index| definition.nodes[index].id.clone())
                        .collect(),
                ));

                return Err(errors);
            }
        };

        let mut ancestors = vec![0u32; definition.nodes.len()];
        let mut depth = vec![0usize; definition.nodes.len()];

        for &node in &order {
            for &dependency in &needs[node] {
                ancestors[node] |= ancestors[dependency] | (1 << dependency);
                depth[node] = depth[node].max(depth[dependency] + 1);
            }
        }

        for (index, template) in templates.iter().enumerate() {
            let Some(template) = template else { continue };

            for reference in template.outputs() {
                let ancestor = nodes
                    .get(reference)
                    .is_some_and(|&other| ancestors[index] & (1 << other) != 0);

                if !ancestor {
                    errors.push(DefinitionError::NotAncestor {
                        node: definition.nodes[index].id.clone(),
                        reference: reference.to_owned(),
                    });
                }
            }
        }

        for first in 0..definition.nodes.len() {
            for second in first + 1..definition.nodes.len() {
                let ordered =
                    ancestors[second] & (1 << first) != 0 || ancestors[first] & (1 << second) != 0;

                if slot_of[first] == slot_of[second] && slot_of[first] != usize::MAX && !ordered {
                    errors.push(DefinitionError::UnorderedSlot {
                        slot: definition.nodes[first].slot.clone(),
                        first: definition.nodes[first].id.clone(),
                        second: definition.nodes[second].id.clone(),
                    });
                }
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(Self {
            definition,
            slot_of,
            needs,
            ancestors,
            templates,
            order,
            depth,
        })
    }

    pub fn definition(&self) -> &Definition {
        &self.definition
    }

    pub fn node_count(&self) -> usize {
        self.definition.nodes.len()
    }

    pub fn slot_count(&self) -> usize {
        self.definition.slots.len()
    }

    pub fn max_parallel(&self) -> usize {
        self.definition
            .max_parallel
            .map_or(DEFAULT_MAX_PARALLEL, |value| value as usize)
    }

    pub fn slot_of(&self, node: usize) -> usize {
        self.slot_of[node]
    }

    /// The node's dependencies, in the order the definition lists them.
    pub fn needs(&self, node: usize) -> &[usize] {
        &self.needs[node]
    }

    pub fn is_ancestor(&self, ancestor: usize, node: usize) -> bool {
        self.ancestors[node] & (1 << ancestor) != 0
    }

    pub fn template(&self, node: usize) -> Option<&Template> {
        self.templates[node].as_ref()
    }

    /// Every node after all of its dependencies; ties keep definition order.
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// The length of the longest dependency chain ending at the node, which
    /// is the display column the node belongs to.
    pub fn depth(&self, node: usize) -> usize {
        self.depth[node]
    }

    pub fn node_index(&self, id: &str) -> Option<usize> {
        self.definition.nodes.iter().position(|node| node.id == id)
    }

    /// Whether some node's prompt contains the run input: a node without
    /// dependencies or template sends the input as its whole prompt.
    pub fn uses_input(&self) -> bool {
        (0..self.node_count()).any(|node| match &self.templates[node] {
            Some(template) => template.uses_input(),
            None => self.needs[node].is_empty(),
        })
    }
}

fn index_slots<'a>(
    definition: &'a Definition,
    errors: &mut Vec<DefinitionError>,
) -> BTreeMap<&'a str, usize> {
    let mut slots = BTreeMap::new();

    for (index, slot) in definition.slots.iter().enumerate() {
        if slot.name.is_empty() {
            errors.push(DefinitionError::EmptySlotName);
        } else if slots.insert(slot.name.as_str(), index).is_some() {
            errors.push(DefinitionError::DuplicateSlot(slot.name.clone()));
        }
    }

    slots
}

fn index_nodes<'a>(
    definition: &'a Definition,
    errors: &mut Vec<DefinitionError>,
) -> BTreeMap<&'a str, usize> {
    if definition.nodes.is_empty() {
        errors.push(DefinitionError::NoNodes);
    }

    if definition.nodes.len() > MAX_NODES {
        errors.push(DefinitionError::TooManyNodes(definition.nodes.len()));
    }

    let mut nodes = BTreeMap::new();

    for (index, node) in definition.nodes.iter().enumerate() {
        if !is_node_id(&node.id) {
            errors.push(DefinitionError::InvalidNodeId(node.id.clone()));
        }

        if nodes.insert(node.id.as_str(), index).is_some() {
            errors.push(DefinitionError::DuplicateNode(node.id.clone()));
        }
    }

    nodes
}

/// Kahn's algorithm, always taking the earliest ready node so the order is
/// stable under edits elsewhere in the file. On a cycle, returns one cycle,
/// each node followed by one of its dependencies.
fn topological_order(needs: &[Vec<usize>]) -> Result<Vec<usize>, Vec<usize>> {
    let mut waiting: Vec<usize> = needs.iter().map(Vec::len).collect();
    let mut done = vec![false; needs.len()];
    let mut order = Vec::with_capacity(needs.len());

    while let Some(next) = (0..needs.len()).find(|&node| !done[node] && waiting[node] == 0) {
        done[next] = true;

        order.push(next);

        for (node, dependencies) in needs.iter().enumerate() {
            if dependencies.contains(&next) {
                waiting[node] -= 1;
            }
        }
    }

    if order.len() == needs.len() {
        return Ok(order);
    }

    // Every node left has a dependency that is also left, so following
    // dependencies from any of them must revisit a node.
    let start = (0..needs.len()).find(|&node| !done[node]).unwrap_or(0);

    let mut path = vec![start];

    loop {
        let current = *path.last().unwrap_or(&start);

        let next = needs[current]
            .iter()
            .copied()
            .find(|&dependency| !done[dependency])
            .unwrap_or(start);

        if let Some(position) = path.iter().position(|&node| node == next) {
            return Err(path.split_off(position));
        }

        path.push(next);
    }
}
