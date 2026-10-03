use serde_json::{Value, json};

use crate::orchestration::definition::{Definition, Position};
use crate::orchestration::placement::{COLUMN_WIDTH, ROW_HEIGHT, place};

fn definition(nodes: Value, layout: Value) -> Definition {
    serde_json::from_value(json!({
        "version": 1,
        "slots": { "dev": { "profile": { "kind": "claude", "name": "Default" } } },
        "nodes": nodes,
        "layout": layout,
    }))
    .unwrap()
}

fn at(x: f32, y: f32) -> Position {
    Position { x, y }
}

/// `plan`, then `frontend` and `backend`, then `review`.
fn diamond() -> Value {
    json!([
        { "id": "plan", "slot": "dev" },
        { "id": "frontend", "slot": "dev", "needs": ["plan"] },
        { "id": "backend", "slot": "dev", "needs": ["plan"] },
        { "id": "review", "slot": "dev", "needs": ["frontend", "backend"] },
    ])
}

#[test]
fn nodes_without_positions_go_to_their_depth_column() {
    let positions = place(&definition(diamond(), json!({})));

    assert_eq!(positions[1].x - positions[0].x, COLUMN_WIDTH);
    assert_eq!(positions[2].x, positions[1].x);
    assert_eq!(positions[2].y - positions[1].y, ROW_HEIGHT);
    assert_eq!(positions[3].x - positions[1].x, COLUMN_WIDTH);
    assert_eq!(positions[3].y, positions[0].y);
}

#[test]
fn stored_positions_are_kept() {
    let positions = place(&definition(
        diamond(),
        json!({ "review": { "x": 900.0, "y": 40.0 } }),
    ));

    assert_eq!(positions[3], at(900., 40.));
}

#[test]
fn placed_nodes_skip_cells_covered_by_stored_ones() {
    let computed = place(&definition(diamond(), json!({})));

    // `frontend` is moved onto the cell `backend` would take.
    let moved = place(&definition(
        diamond(),
        json!({ "frontend": { "x": computed[2].x, "y": computed[2].y } }),
    ));

    // `backend` takes the free first cell of its column, not `frontend`'s.
    assert_eq!(moved[1], computed[2]);
    assert_eq!(moved[2], computed[1]);

    // With both cells of the column taken, the next placed node goes below.
    let crowded = place(&definition(
        diamond(),
        json!({
            "frontend": { "x": computed[1].x, "y": computed[1].y },
            "review": { "x": computed[2].x, "y": computed[2].y },
        }),
    ));

    assert_eq!(crowded[2].x, computed[2].x);
    assert_eq!(crowded[2].y - computed[2].y, ROW_HEIGHT);
}

#[test]
fn nodes_on_a_cycle_go_to_the_first_column() {
    let positions = place(&definition(
        json!([
            { "id": "start", "slot": "dev" },
            { "id": "a", "slot": "dev", "needs": ["b"] },
            { "id": "b", "slot": "dev", "needs": ["a", "missing"] },
        ]),
        json!({}),
    ));

    assert_eq!(positions[1].x, positions[0].x);
    assert_eq!(positions[2].x, positions[0].x);
    assert!(positions[0].y < positions[1].y && positions[1].y < positions[2].y);
}
