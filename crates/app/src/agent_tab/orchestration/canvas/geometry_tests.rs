use gpui::{point, px};
use nmt_agent::orchestration::definition::Position;

use crate::agent_tab::orchestration::canvas::geometry::{
    CARD_HEIGHT, CARD_WIDTH, hit_card, hit_edge, hit_port, port_center,
};
use crate::agent_tab::orchestration::canvas::viewport::Viewport;

fn at(x: f32, y: f32) -> Position {
    Position { x, y }
}

#[test]
fn the_topmost_card_under_the_pointer_is_hit() {
    let viewport = Viewport::default();
    let positions = [at(0., 0.), at(100., 50.)];

    assert_eq!(
        hit_card(point(px(10.), px(10.)), &positions, viewport),
        Some(0)
    );
    assert_eq!(
        hit_card(point(px(150.), px(60.)), &positions, viewport),
        Some(1)
    );
    assert_eq!(
        hit_card(point(px(10.), px(500.)), &positions, viewport),
        None
    );
}

#[test]
fn a_port_keeps_its_screen_size_when_zoomed_out() {
    let viewport = Viewport {
        offset: point(px(0.), px(0.)),
        zoom: 0.25,
    };

    let positions = [at(0., 0.)];
    let center = port_center(positions[0], viewport);

    assert_eq!(center, point(px(CARD_WIDTH / 4.), px(CARD_HEIGHT / 8.)));
    assert_eq!(
        hit_port(center + point(px(5.), px(0.)), &positions, viewport),
        Some(0)
    );
    assert_eq!(
        hit_port(center + point(px(20.), px(0.)), &positions, viewport),
        None
    );
}

#[test]
fn an_edge_is_hit_along_its_curve_and_not_beside_it() {
    let viewport = Viewport::default();

    // Side by side at the same height, the curve is a straight line from
    // the first card's port to the second card's left edge.
    let positions = [at(0., 0.), at(400., 0.)];
    let edges = [(0, 1)];
    let middle_y = CARD_HEIGHT / 2.;
    let middle_x = (CARD_WIDTH + 400.) / 2.;

    assert_eq!(
        hit_edge(
            point(px(middle_x), px(middle_y + 3.)),
            &edges,
            &positions,
            viewport
        ),
        Some(0)
    );
    assert_eq!(
        hit_edge(
            point(px(middle_x), px(middle_y + 20.)),
            &edges,
            &positions,
            viewport
        ),
        None
    );
}
