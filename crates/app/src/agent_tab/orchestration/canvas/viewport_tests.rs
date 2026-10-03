use gpui::{point, px, size};
use nmt_agent::orchestration::definition::Position;

use crate::agent_tab::orchestration::canvas::viewport::{Gesture, MAX_ZOOM, MIN_ZOOM, Viewport};

fn at(x: f32, y: f32) -> Position {
    Position { x, y }
}

#[test]
fn screen_and_canvas_points_convert_both_ways() {
    let viewport = Viewport {
        offset: point(px(100.), px(50.)),
        zoom: 2.,
    };

    assert_eq!(viewport.to_screen(at(10., 20.)), point(px(120.), px(90.)));
    assert_eq!(viewport.to_canvas(point(px(120.), px(90.))), at(10., 20.));
}

#[test]
fn zooming_keeps_the_point_under_the_pointer() {
    let mut viewport = Viewport::default();

    let pointer = point(px(300.), px(200.));
    let under = viewport.to_canvas(pointer);

    viewport.zoom_at(pointer, 1.5);

    assert_eq!(viewport.zoom, 1.5);
    assert_eq!(viewport.to_screen(under), pointer);
}

#[test]
fn zoom_stays_within_its_limits() {
    let mut viewport = Viewport::default();

    viewport.zoom_at(point(px(0.), px(0.)), 100.);

    assert_eq!(viewport.zoom, MAX_ZOOM);

    viewport.zoom_at(point(px(0.), px(0.)), 0.0001);

    assert_eq!(viewport.zoom, MIN_ZOOM);
}

#[test]
fn fitting_shows_every_card_without_zooming_past_full_size() {
    let card = size(180., 96.);
    let area = size(px(800.), px(600.));
    let positions = [at(0., 0.), at(2000., 0.), at(1000., 1500.)];

    let viewport = Viewport::fit(&positions, card, area);

    for position in positions {
        let top_left = viewport.to_screen(position);

        let bottom_right =
            viewport.to_screen(at(position.x + card.width, position.y + card.height));

        assert!(top_left.x >= px(0.) && top_left.y >= px(0.), "{top_left:?}");
        assert!(
            bottom_right.x <= area.width && bottom_right.y <= area.height,
            "{bottom_right:?}"
        );
    }

    let small = Viewport::fit(&[at(0., 0.)], card, area);

    assert_eq!(small.zoom, 1.);
}

#[test]
fn a_pan_follows_the_pointer_and_ends_on_release() {
    let mut gesture = Gesture::press(point(px(10.), px(10.)));

    assert_eq!(
        gesture.moved(point(px(25.), px(5.)), true),
        Some(point(px(15.), px(-5.)))
    );
    assert_eq!(
        gesture.moved(point(px(30.), px(5.)), true),
        Some(point(px(5.), px(0.)))
    );

    gesture.release();

    assert_eq!(gesture, Gesture::Idle);
    assert_eq!(gesture.moved(point(px(40.), px(5.)), true), None);
}

#[test]
fn a_pan_released_outside_the_canvas_ends_on_the_next_move() {
    let mut gesture = Gesture::press(point(px(10.), px(10.)));

    assert_eq!(gesture.moved(point(px(20.), px(10.)), false), None);
    assert_eq!(gesture, Gesture::Idle);
}
