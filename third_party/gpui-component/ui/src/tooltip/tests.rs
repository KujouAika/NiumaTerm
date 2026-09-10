use crate::tooltip::*;
use gpui::size;

fn test_content(bounds: Bounds<Pixels>) -> TooltipContent {
    TooltipContent {
        build: Rc::new(|window, cx| Tooltip::new("Test tooltip").build(window, cx)),
        trigger_bounds: bounds,
        anchor: TooltipAnchor::Cursor,
    }
}

fn test_bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
}

fn test_size(width: f32, height: f32) -> Size<Pixels> {
    size(px(width), px(height))
}

#[test]
fn tooltip_overlay_clear_state_resets_active_tooltip() {
    let mut overlay = TooltipOverlay::new();

    overlay.content = Some(test_content(test_bounds(10., 10., 40., 20.)));
    overlay.prev_trigger_bounds = Some(test_bounds(0., 0., 40., 20.));
    overlay.had_recent_tooltip = true;
    overlay.is_switching = true;
    overlay._show_task = Some(Task::ready(()));

    assert!(overlay.clear_state());
    assert!(overlay.content.is_none());
    assert!(overlay.prev_trigger_bounds.is_none());
    assert!(!overlay.had_recent_tooltip);
    assert!(!overlay.is_switching);
    assert!(overlay._show_task.is_none());
    assert!(overlay._hide_task.is_none());
}

#[test]
fn tooltip_overlay_position_prefers_above_when_space_allows() {
    let trigger_bounds = test_bounds(100., 80., 80., 24.);
    let position = tooltip_overlay_position(
        trigger_bounds,
        test_size(120., 30.),
        test_size(300., 200.),
        TOOLTIP_WINDOW_MARGIN,
        TooltipAnchor::Cursor,
    );

    assert_eq!(position.placement, TooltipPlacement::Above);
    assert_eq!(position.bounds.origin.x, trigger_bounds.left());
    assert_eq!(position.bounds.origin.y, px(50.));
    assert_eq!(position.bounds.bottom(), trigger_bounds.top());
}

#[test]
fn tooltip_overlay_position_flips_below_near_top_edge() {
    let trigger_bounds = test_bounds(24., 4., 120., 32.);
    let position = tooltip_overlay_position(
        trigger_bounds,
        test_size(240., 32.),
        test_size(520., 260.),
        TOOLTIP_WINDOW_MARGIN,
        TooltipAnchor::Cursor,
    );

    assert_eq!(position.placement, TooltipPlacement::Below);
    assert_eq!(position.bounds.top(), trigger_bounds.bottom());
    assert!(position.bounds.top() >= trigger_bounds.bottom());
}

#[test]
fn tooltip_overlay_position_clamps_horizontal_edges() {
    let trigger_bounds = test_bounds(4., 80., 24., 24.);
    let position = tooltip_overlay_position(
        trigger_bounds,
        test_size(120., 30.),
        test_size(300., 200.),
        TOOLTIP_WINDOW_MARGIN,
        TooltipAnchor::Cursor,
    );

    assert_eq!(position.placement, TooltipPlacement::Above);
    assert_eq!(position.bounds.left(), TOOLTIP_WINDOW_MARGIN);
}

#[test]
fn tooltip_overlay_position_uses_larger_side_when_neither_side_fits() {
    let trigger_bounds = test_bounds(120., 20., 40., 20.);
    let position = tooltip_overlay_position(
        trigger_bounds,
        test_size(160., 120.),
        test_size(300., 100.),
        TOOLTIP_WINDOW_MARGIN,
        TooltipAnchor::Cursor,
    );

    assert_eq!(position.placement, TooltipPlacement::Below);
    assert_eq!(position.bounds.top(), TOOLTIP_WINDOW_MARGIN);
    assert_eq!(position.bounds.left(), trigger_bounds.left());
}

struct TooltipHost;

impl Render for TooltipHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().children((0..2usize).map(|row| {
            div()
                .id(("directory", row))
                .absolute()
                .left(px(20.))
                .top(px(100. + row as f32 * 44.))
                .w(px(220.))
                .h(px(32.))
                .child("Directory")
                .managed_tooltip_right(if row == 0 {
                    "Primary directory: /Users/test/work".to_string()
                } else {
                    format!(
                        r"Primary directory: C:\Users\test\{}",
                        r"long-directory\".repeat(8)
                    )
                })
        }))
    }
}

#[gpui::test]
fn side_tooltips_keep_hover_timing_and_clear_rows(cx: &mut gpui::TestAppContext) {
    cx.update(crate::init);
    let (root, cx) = cx.add_window_view(|window, cx| {
        let host = cx.new(|_| TooltipHost);
        Root::new(host, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(700.)));
    cx.run_until_parked();
    cx.refresh().unwrap();
    let overlay = root.read_with(cx, |root, _| root.tooltip_overlay.clone());
    cx.simulate_mouse_move(point(px(60.), px(116.)), None, gpui::Modifiers::default());
    cx.executor().advance_clock(Duration::from_millis(499));
    cx.run_until_parked();
    assert!(
        !overlay.read_with(cx, |overlay, _| overlay.has_content()),
        "first hover must retain its delay"
    );
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert!(overlay.read_with(cx, |overlay, _| overlay.has_content()));
    cx.executor().advance_clock(Duration::from_millis(160));
    cx.run_until_parked();
    cx.refresh().unwrap();
    let short = cx
        .debug_bounds("tooltip-view")
        .expect("tooltip was not painted");
    assert_eq!(short.left(), px(248.));
    assert!(short.size.width < SIDE_TOOLTIP_MAX_WIDTH);

    cx.simulate_mouse_move(point(px(60.), px(160.)), None, gpui::Modifiers::default());
    cx.run_until_parked();
    let active = overlay.read_with(cx, |overlay, _| overlay.content.clone().unwrap());
    assert_eq!(
        active.trigger_bounds.top(),
        px(144.),
        "switching rows must show the new content immediately"
    );
    cx.refresh().unwrap();
    let wide = cx.debug_bounds("tooltip-view").unwrap();
    assert_eq!(wide.left(), px(248.));
    assert!(wide.size.width > px(420.));
    assert!(wide.size.width <= SIDE_TOOLTIP_MAX_WIDTH);

    cx.simulate_resize(size(px(640.), px(700.)));
    cx.run_until_parked();
    cx.refresh().unwrap();
    let narrow = cx.debug_bounds("tooltip-view").unwrap();
    assert_eq!(narrow.left(), px(248.));
    assert!(
        narrow.right() <= px(636.),
        "tooltip must fit without shifting across its row: {narrow:?}"
    );
    assert!(
        narrow.size.height > wide.size.height,
        "long paths wrap when side space is reduced"
    );
}
