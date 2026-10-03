//! The canvas transform and the pointer gestures that change it.
//!
//! Canvas positions are logical pixels at 100% zoom; screen points are
//! relative to the canvas element's origin. Both are plain values so the
//! transform and the gesture transitions are unit-testable without a window.

use gpui::{Pixels, Point, Size, point, px};
use nmt_agent::orchestration::definition::Position;

pub(super) const MIN_ZOOM: f32 = 0.25;
pub(super) const MAX_ZOOM: f32 = 2.;

/// Space kept around the nodes when fitting them into view.
const FIT_PADDING: f32 = 32.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Viewport {
    pub(super) offset: Point<Pixels>,
    pub(super) zoom: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            offset: point(px(0.), px(0.)),
            zoom: 1.,
        }
    }
}

impl Viewport {
    pub(super) fn to_screen(self, position: Position) -> Point<Pixels> {
        point(
            self.offset.x + px(position.x * self.zoom),
            self.offset.y + px(position.y * self.zoom),
        )
    }

    pub(super) fn to_canvas(self, screen: Point<Pixels>) -> Position {
        Position {
            x: f32::from(screen.x - self.offset.x) / self.zoom,
            y: f32::from(screen.y - self.offset.y) / self.zoom,
        }
    }

    pub(super) fn pan(&mut self, delta: Point<Pixels>) {
        self.offset += delta;
    }

    /// Scale by `factor`, keeping the canvas point under `anchor` where it is
    /// on screen.
    pub(super) fn zoom_at(&mut self, anchor: Point<Pixels>, factor: f32) {
        let fixed = self.to_canvas(anchor);

        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);

        self.offset = point(
            anchor.x - px(fixed.x * self.zoom),
            anchor.y - px(fixed.y * self.zoom),
        );
    }

    /// The viewport that shows every card, each `card` large at 100% zoom
    /// with its top-left corner at a position, centered in `area`. It never
    /// zooms in past 100%, so a small graph is not blown up.
    pub(super) fn fit(positions: &[Position], card: Size<f32>, area: Size<Pixels>) -> Self {
        let Some(first) = positions.first() else {
            return Self::default();
        };

        let (mut left, mut top) = (first.x, first.y);
        let (mut right, mut bottom) = (first.x + card.width, first.y + card.height);

        for position in positions {
            left = left.min(position.x);
            top = top.min(position.y);
            right = right.max(position.x + card.width);
            bottom = bottom.max(position.y + card.height);
        }

        let width = (right - left).max(1.);
        let height = (bottom - top).max(1.);
        let room_x = (f32::from(area.width) - 2. * FIT_PADDING).max(1.);
        let room_y = (f32::from(area.height) - 2. * FIT_PADDING).max(1.);

        let zoom = (room_x / width)
            .min(room_y / height)
            .min(1.)
            .clamp(MIN_ZOOM, MAX_ZOOM);

        let offset = point(
            px((f32::from(area.width) - width * zoom) / 2. - left * zoom),
            px((f32::from(area.height) - height * zoom) / 2. - top * zoom),
        );

        Self { offset, zoom }
    }
}

/// What the pointer is doing on the canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Gesture {
    #[default]
    Idle,
    Panning {
        last: Point<Pixels>,
    },
}

impl Gesture {
    /// The primary button went down on the empty background at `at`.
    pub(super) fn press(at: Point<Pixels>) -> Self {
        Self::Panning { last: at }
    }

    /// The pointer moved to `at` with the primary button `held` or not.
    /// Returns how far to pan; a move without the button ends a pan whose
    /// release happened outside the canvas.
    pub(super) fn moved(&mut self, at: Point<Pixels>, held: bool) -> Option<Point<Pixels>> {
        match *self {
            Self::Panning { last } if held => {
                *self = Self::Panning { last: at };

                Some(at - last)
            }
            Self::Panning { .. } => {
                *self = Self::Idle;

                None
            }
            Self::Idle => None,
        }
    }

    pub(super) fn release(&mut self) {
        *self = Self::Idle;
    }
}
