//! Canvas positions for a definition's nodes.
//!
//! A node with a stored position keeps it. Any other node is placed on a
//! grid: one column per dependency depth, rows in definition order, skipping
//! cells a stored position already covers so a placed node never hides one
//! the user moved. Placement works on definitions that do not
//! validate, because the canvas edits graphs through invalid states.

#[cfg(test)]
#[path = "placement_tests.rs"]
mod placement_tests;

use std::collections::BTreeMap;

use crate::orchestration::definition::{Definition, Position};

/// Horizontal distance between depth columns.
pub const COLUMN_WIDTH: f32 = 240.;

/// Vertical distance between rows of a column.
pub const ROW_HEIGHT: f32 = 130.;

/// Space between the canvas origin and the first cell.
const MARGIN: f32 = 24.;

/// One position per node, in definition order.
pub fn place(definition: &Definition) -> Vec<Position> {
    let depth = depths(definition);

    let stored: Vec<Position> = definition
        .nodes
        .iter()
        .filter_map(|node| definition.layout.get(&node.id).copied())
        .collect();

    let mut next_row: BTreeMap<usize, usize> = BTreeMap::new();

    definition
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            if let Some(position) = definition.layout.get(&node.id) {
                return *position;
            }

            let column = depth[index];
            let row = next_row.entry(column).or_insert(0);

            loop {
                let cell = cell(column, *row);

                *row += 1;

                if !stored.iter().any(|taken| covers(*taken, cell)) {
                    return cell;
                }
            }
        })
        .collect()
}

fn cell(column: usize, row: usize) -> Position {
    Position {
        x: MARGIN + column as f32 * COLUMN_WIDTH,
        y: MARGIN + row as f32 * ROW_HEIGHT,
    }
}

/// Whether a card at `taken` overlaps the grid cell at `cell`.
fn covers(taken: Position, cell: Position) -> bool {
    (taken.x - cell.x).abs() < COLUMN_WIDTH / 2. && (taken.y - cell.y).abs() < ROW_HEIGHT / 2.
}

/// The length of the longest dependency chain ending at each node. Unknown
/// dependencies are skipped, and a node on a cycle or depending on one has
/// no defined depth and goes to the first column.
fn depths(definition: &Definition) -> Vec<usize> {
    let index: BTreeMap<&str, usize> = definition
        .nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (node.id.as_str(), position))
        .collect();

    let needs: Vec<Vec<usize>> = definition
        .nodes
        .iter()
        .map(|node| {
            node.needs
                .iter()
                .filter_map(|id| index.get(id.as_str()).copied())
                .collect()
        })
        .collect();

    let mut depth = vec![None; needs.len()];

    // Each pass settles every node whose dependencies all have a depth, so
    // the passes end once nothing changes; whatever is left is on a cycle.
    loop {
        let mut changed = false;

        for node in 0..needs.len() {
            if depth[node].is_some() {
                continue;
            }

            let known: Option<Vec<usize>> = needs[node]
                .iter()
                .map(|&dependency| depth[dependency])
                .collect();

            if let Some(known) = known {
                depth[node] = Some(known.iter().map(|value| value + 1).max().unwrap_or(0));
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    depth.into_iter().map(|value| value.unwrap_or(0)).collect()
}
