//! Where cards, ports and edges are on screen, and what a point hits.
//!
//! Every point here is relative to the canvas element's origin. Painting and
//! hit testing read the same curve, so an edge is selectable exactly where
//! it is drawn.

use gpui::{Bounds, Pixels, Point, point, px, size};
use nmt_agent::orchestration::definition::Position;

use crate::agent_tab::orchestration::canvas::viewport::Viewport;

/// A card's size at 100% zoom; placement spaces cells wider and taller so
/// neighbouring cards keep a gap.
pub(super) const CARD_WIDTH: f32 = 180.;

pub(super) const CARD_HEIGHT: f32 = 96.;

/// Radius of a card's output port, in screen pixels at any zoom, so it stays
/// grabbable when zoomed out.
pub(super) const PORT_RADIUS: f32 = 6.;

/// How close to an edge a click must be to select it.
const EDGE_TOLERANCE: f32 = 6.;

/// Straight segments an edge's curve is split into for hit testing.
const EDGE_SAMPLES: usize = 24;

pub(super) fn card_bounds(position: Position, viewport: Viewport) -> Bounds<Pixels> {
    Bounds::new(
        viewport.to_screen(position),
        size(
            px(CARD_WIDTH * viewport.zoom),
            px(CARD_HEIGHT * viewport.zoom),
        ),
    )
}

/// Where a card's output port is: the middle of its right edge.
pub(super) fn port_center(position: Position, viewport: Viewport) -> Point<Pixels> {
    viewport.to_screen(Position {
        x: position.x + CARD_WIDTH,
        y: position.y + CARD_HEIGHT / 2.,
    })
}

/// Where an edge enters a card: the middle of its left edge.
pub(super) fn input_point(position: Position, viewport: Viewport) -> Point<Pixels> {
    viewport.to_screen(Position {
        x: position.x,
        y: position.y + CARD_HEIGHT / 2.,
    })
}

/// A cubic curve from `start` to `end` that leaves and enters horizontally:
/// start, first control, second control, end.
pub(super) fn edge_curve(
    start: Point<Pixels>,
    end: Point<Pixels>,
    zoom: f32,
) -> [Point<Pixels>; 4] {
    let reach = ((end.x - start.x).abs() / 2.).max(px(40. * zoom));

    [
        start,
        point(start.x + reach, start.y),
        point(end.x - reach, end.y),
        end,
    ]
}

/// The topmost card under `at`; later cards are drawn over earlier ones.
pub(super) fn hit_card(
    at: Point<Pixels>,
    positions: &[Position],
    viewport: Viewport,
) -> Option<usize> {
    positions
        .iter()
        .rposition(|position| card_bounds(*position, viewport).contains(&at))
}

/// The card whose output port is under `at`.
pub(super) fn hit_port(
    at: Point<Pixels>,
    positions: &[Position],
    viewport: Viewport,
) -> Option<usize> {
    positions
        .iter()
        .rposition(|position| distance(at, port_center(*position, viewport)) <= PORT_RADIUS + 2.)
}

/// The edge, by index into `edges`, that passes within a few pixels of `at`.
pub(super) fn hit_edge(
    at: Point<Pixels>,
    edges: &[(usize, usize)],
    positions: &[Position],
    viewport: Viewport,
) -> Option<usize> {
    edges.iter().position(|&(from, to)| {
        let (Some(from), Some(to)) = (positions.get(from), positions.get(to)) else {
            return false;
        };

        let curve = edge_curve(
            port_center(*from, viewport),
            input_point(*to, viewport),
            viewport.zoom,
        );

        let samples: Vec<Point<Pixels>> = (0..=EDGE_SAMPLES)
            .map(|step| cubic(curve, step as f32 / EDGE_SAMPLES as f32))
            .collect();

        samples
            .windows(2)
            .any(|pair| segment_distance(at, pair[0], pair[1]) <= EDGE_TOLERANCE)
    })
}

fn cubic(curve: [Point<Pixels>; 4], t: f32) -> Point<Pixels> {
    let u = 1. - t;
    let weights = [u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t];

    let x = curve
        .iter()
        .zip(weights)
        .map(|(point, weight)| f32::from(point.x) * weight)
        .sum::<f32>();

    let y = curve
        .iter()
        .zip(weights)
        .map(|(point, weight)| f32::from(point.y) * weight)
        .sum::<f32>();

    point(px(x), px(y))
}

pub(super) fn distance(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    f32::from(a.x - b.x).hypot(f32::from(a.y - b.y))
}

fn segment_distance(at: Point<Pixels>, a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    let (ax, ay) = (f32::from(a.x), f32::from(a.y));
    let (dx, dy) = (f32::from(b.x) - ax, f32::from(b.y) - ay);
    let length = dx * dx + dy * dy;

    if length == 0. {
        return distance(at, a);
    }

    let t = (((f32::from(at.x) - ax) * dx + (f32::from(at.y) - ay) * dy) / length).clamp(0., 1.);

    distance(at, point(px(ax + t * dx), px(ay + t * dy)))
}
