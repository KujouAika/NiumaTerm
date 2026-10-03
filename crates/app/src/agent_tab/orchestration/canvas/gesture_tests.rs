use gpui::{Pixels, Point, point, px};

use crate::agent_tab::orchestration::canvas::gesture::{Gesture, Moved, Released, Target};

fn at(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

#[test]
fn a_pan_follows_the_pointer_and_ends_on_release() {
    let mut gesture = Gesture::press(Target::Background, at(10., 10.), true);

    assert_eq!(gesture.moved(at(25., 5.), true), Moved::Pan(at(15., -5.)));
    assert_eq!(gesture.moved(at(30., 5.), true), Moved::Pan(at(5., 0.)));
    assert_eq!(gesture.release(at(30., 5.), None), Released::Nothing);
    assert_eq!(gesture, Gesture::Idle);
    assert_eq!(gesture.moved(at(40., 5.), true), Moved::Nothing);
}

#[test]
fn a_press_on_the_background_that_barely_moves_is_a_click() {
    let mut gesture = Gesture::press(Target::Background, at(10., 10.), true);

    gesture.moved(at(11., 11.), true);

    assert_eq!(
        gesture.release(at(11., 11.), None),
        Released::BackgroundClick(at(11., 11.))
    );
}

#[test]
fn a_gesture_released_outside_the_canvas_ends_on_the_next_move() {
    let mut gesture = Gesture::press(Target::Card(0), at(10., 10.), true);

    gesture.moved(at(40., 10.), true);

    assert_eq!(gesture.moved(at(50., 10.), false), Moved::Redraw);
    assert_eq!(gesture, Gesture::Idle);
    assert_eq!(gesture.release(at(50., 10.), None), Released::Nothing);
}

#[test]
fn a_card_moves_only_past_the_threshold() {
    let mut gesture = Gesture::press(Target::Card(2), at(10., 10.), true);

    assert_eq!(gesture.moved(at(12., 11.), true), Moved::Nothing);
    assert_eq!(gesture.drag_offset(2), None);
    assert_eq!(
        gesture.release(at(12., 11.), Some(2)),
        Released::CardClick(2)
    );

    let mut gesture = Gesture::press(Target::Card(2), at(10., 10.), true);

    assert_eq!(gesture.moved(at(30., 10.), true), Moved::Redraw);
    assert_eq!(gesture.drag_offset(2), Some(at(20., 0.)));
    assert_eq!(gesture.drag_offset(1), None);
    assert_eq!(
        gesture.release(at(35., 15.), Some(2)),
        Released::Dragged {
            node: 2,
            by: at(25., 5.),
        }
    );
}

#[test]
fn a_connection_completes_only_over_another_card() {
    let mut gesture = Gesture::press(Target::Port(0), at(0., 0.), true);

    assert_eq!(gesture.moved(at(50., 50.), true), Moved::Redraw);
    assert_eq!(
        gesture.release(at(50., 50.), Some(1)),
        Released::Connected { from: 0, to: 1 }
    );

    let mut gesture = Gesture::press(Target::Port(0), at(0., 0.), true);

    assert_eq!(gesture.release(at(5., 5.), Some(0)), Released::Nothing);

    let mut gesture = Gesture::press(Target::Port(0), at(0., 0.), true);

    assert_eq!(gesture.release(at(500., 5.), None), Released::Nothing);
}

#[test]
fn a_read_only_canvas_ignores_presses_on_cards_and_ports() {
    assert_eq!(
        Gesture::press(Target::Card(0), at(0., 0.), false),
        Gesture::Idle
    );
    assert_eq!(
        Gesture::press(Target::Port(0), at(0., 0.), false),
        Gesture::Idle
    );
}
