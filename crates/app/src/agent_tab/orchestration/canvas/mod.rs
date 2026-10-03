//! The orchestration canvas: node cards placed by a viewport transform, and
//! one painted layer of dependency edges drawn as cubic curves.
//!
//! Cards are plain GPUI elements so text layout and clicks work as anywhere
//! else; only the edges are custom-painted. Pointer events arrive outside a
//! frame, so every handler that changes what is shown calls `cx.notify`.

mod viewport;

#[cfg(test)]
#[path = "viewport_tests.rs"]
mod viewport_tests;

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    Bounds, Context, EventEmitter, FontWeight, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathBuilder, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Window,
    canvas, div, point, px, size,
};
use gpui_component::ActiveTheme as _;
use nmt_agent::orchestration::definition::Position;
use rust_i18n::t;

use crate::agent_tab::orchestration::canvas::viewport::{Gesture, Viewport};
use crate::agent_tab::settings::UI_RADIUS;

/// A card's size at 100% zoom; placement spaces cells wider and taller so
/// neighbouring cards keep a gap.
const CARD_WIDTH: f32 = 180.;

const CARD_HEIGHT: f32 = 96.;

/// Below this zoom a card shows only its id; its state line would be too
/// small to read.
const DETAIL_ZOOM: f32 = 0.5;

/// Zoom change per scrolled line with Ctrl held.
const ZOOM_STEP: f32 = 1.1;

/// Pixels of a precise scroll that count as one line of zoom.
const PIXELS_PER_ZOOM_STEP: f32 = 50.;

/// A node's run state, as the card shows it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CardState {
    pub(super) label: String,
    pub(super) color: Hsla,
    pub(super) needs_input: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CanvasNode {
    pub(super) id: String,
    pub(super) position: Position,

    /// Present on a run's canvas, which also offers each node's details.
    pub(super) state: Option<CardState>,
}

pub(super) enum CanvasEvent {
    OpenDetails(usize),
}

pub(super) struct GraphCanvas {
    /// Which graph is shown; a new one is fitted into view once laid out.
    key: String,

    nodes: Vec<CanvasNode>,

    /// Dependency edges as (dependency, dependent) node indices.
    edges: Vec<(usize, usize)>,

    viewport: Viewport,
    gesture: Gesture,

    /// The element's bounds from its last layout, for fitting and for
    /// turning window positions into canvas-relative ones.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,

    fit_pending: bool,
}

impl EventEmitter<CanvasEvent> for GraphCanvas {}

impl GraphCanvas {
    pub(super) fn new() -> Self {
        Self {
            key: String::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
            viewport: Viewport::default(),
            gesture: Gesture::Idle,
            bounds: Rc::new(Cell::new(None)),
            fit_pending: true,
        }
    }

    /// Show `nodes` and `edges` as the graph named `key`. Showing a
    /// different graph fits it into view; refreshing the same one keeps the
    /// user's pan and zoom.
    pub(super) fn show(
        &mut self,
        key: &str,
        nodes: Vec<CanvasNode>,
        edges: Vec<(usize, usize)>,
        cx: &mut Context<Self>,
    ) {
        if self.key != key {
            self.key = key.to_owned();
            self.fit_pending = true;
        }

        if self.nodes != nodes || self.edges != edges {
            self.nodes = nodes;
            self.edges = edges;

            cx.notify();
        }
    }

    /// Fit every node into view on the next render.
    pub(super) fn fit(&mut self, cx: &mut Context<Self>) {
        self.fit_pending = true;

        cx.notify();
    }

    fn local(&self, window_position: Point<Pixels>) -> Point<Pixels> {
        let origin = self
            .bounds
            .get()
            .map_or(point(px(0.), px(0.)), |bounds| bounds.origin);

        window_position - origin
    }

    fn on_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.gesture = Gesture::press(self.local(event.position));

        cx.notify();
    }

    fn on_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let held = event.pressed_button == Some(MouseButton::Left);

        if let Some(delta) = self.gesture.moved(self.local(event.position), held) {
            self.viewport.pan(delta);

            cx.notify();
        }
    }

    fn on_up(&mut self, cx: &mut Context<Self>) {
        self.gesture.release();

        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, window: &Window, cx: &mut Context<Self>) {
        if event.modifiers.control {
            let lines = match event.delta {
                ScrollDelta::Lines(delta) => delta.y,
                ScrollDelta::Pixels(delta) => f32::from(delta.y) / PIXELS_PER_ZOOM_STEP,
            };

            self.viewport
                .zoom_at(self.local(event.position), ZOOM_STEP.powf(lines));
        } else {
            self.viewport
                .pan(event.delta.pixel_delta(window.line_height()));
        }

        cx.stop_propagation();

        cx.notify();
    }

    fn card(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let node = &self.nodes[index];
        let theme = cx.theme();
        let zoom = self.viewport.zoom;
        let origin = self.viewport.to_screen(node.position);
        let detailed = zoom >= DETAIL_ZOOM;

        let border = match &node.state {
            Some(state) if state.needs_input => theme.warning,
            _ => theme.border,
        };

        div()
            .id(("orchestration-node", index))
            .debug_selector(move || format!("orchestration-node-{index}"))
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .w(px(CARD_WIDTH * zoom))
            .h(px(CARD_HEIGHT * zoom))
            .p(px(8. * zoom))
            .flex()
            .flex_col()
            .gap(px(4. * zoom))
            .overflow_hidden()
            .rounded(UI_RADIUS)
            .border_1()
            .border_color(border)
            .bg(theme.background)
            // A press on a card is not the start of a pan.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(14. * zoom))
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(node.id.clone()),
            )
            .when_some(node.state.clone().filter(|_| detailed), |card, state| {
                card.child(
                    div()
                        .text_size(px(12. * zoom))
                        .text_color(state.color)
                        .truncate()
                        .child(state.label),
                )
                .when(state.needs_input, |card| {
                    card.child(
                        div()
                            .text_size(px(12. * zoom))
                            .text_color(theme.warning)
                            .truncate()
                            .child(t!("orchestration-node-needs-input").into_owned()),
                    )
                })
                .child(
                    div()
                        .id(("orchestration-node-details", index))
                        .text_size(px(12. * zoom))
                        .text_color(theme.link)
                        .cursor_pointer()
                        .child(t!("orchestration-view-details").into_owned())
                        .on_click(
                            cx.listener(move |_, _, _, cx| {
                                cx.emit(CanvasEvent::OpenDetails(index))
                            }),
                        ),
                )
            })
    }
}

impl Render for GraphCanvas {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Fitting needs the element's size, known once it has been laid out;
        // the paint below asks for this render again when it was not.
        if self.fit_pending
            && let Some(bounds) = self.bounds.get()
        {
            let positions: Vec<Position> = self.nodes.iter().map(|node| node.position).collect();

            self.viewport = Viewport::fit(&positions, size(CARD_WIDTH, CARD_HEIGHT), bounds.size);
            self.fit_pending = false;
        }

        let zoom = self.viewport.zoom;
        let edge_color = cx.theme().muted_foreground;

        // Each edge leaves the dependency's right edge and enters the
        // dependent's left edge at mid height, in canvas-relative points.
        let edges: Vec<(Point<Pixels>, Point<Pixels>)> = self
            .edges
            .iter()
            .filter_map(|&(from, to)| {
                let from = self.nodes.get(from)?;
                let to = self.nodes.get(to)?;

                let start = self.viewport.to_screen(Position {
                    x: from.position.x + CARD_WIDTH,
                    y: from.position.y + CARD_HEIGHT / 2.,
                });

                let end = self.viewport.to_screen(Position {
                    x: to.position.x,
                    y: to.position.y + CARD_HEIGHT / 2.,
                });

                Some((start, end))
            })
            .collect();

        let bounds = self.bounds.clone();
        let fit_pending = self.fit_pending;
        let canvas_entity = cx.entity_id();

        let layer = canvas(
            move |layout, _, _| bounds.set(Some(layout)),
            move |layout, _, window, cx| {
                for (start, end) in edges {
                    let (start, end) = (layout.origin + start, layout.origin + end);
                    let reach = ((end.x - start.x).abs() / 2.).max(px(40. * zoom));

                    let mut path = PathBuilder::stroke(px(1.5));

                    path.move_to(start);

                    path.cubic_bezier_to(
                        end,
                        point(start.x + reach, start.y),
                        point(end.x - reach, end.y),
                    );

                    if let Ok(path) = path.build() {
                        window.paint_path(path, edge_color);
                    }
                }

                // This layout is what a pending fit measures, so the canvas
                // renders again to apply it. A view marked dirty during paint
                // is cleared when the frame finishes, so the window asks for
                // it again from a next-frame callback, which a frame queued
                // during paint always runs. The direct mark serves a test
                // that draws frames itself and runs no frame callbacks.
                if fit_pending {
                    cx.notify(canvas_entity);

                    window.on_next_frame(move |_, cx| cx.notify(canvas_entity));
                }
            },
        )
        .absolute()
        .size_full();

        let mut cards = Vec::with_capacity(self.nodes.len());

        for index in 0..self.nodes.len() {
            cards.push(self.card(index, cx).into_any_element());
        }

        div()
            .id("orchestration-canvas")
            .debug_selector(|| "orchestration-canvas".into())
            .relative()
            .size_full()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| this.on_down(event, cx)),
            )
            .on_mouse_move(
                cx.listener(|this, event: &MouseMoveEvent, _, cx| this.on_move(event, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.on_up(cx)),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                this.on_scroll(event, window, cx)
            }))
            .child(layer)
            .children(cards)
            .child(
                div()
                    .id("orchestration-canvas-fit")
                    .absolute()
                    .right(px(8.))
                    .bottom(px(8.))
                    .px_2()
                    .py_1()
                    .rounded(UI_RADIUS)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .text_xs()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx)))
                    .child(t!("orchestration-canvas-fit").into_owned()),
            )
    }
}
