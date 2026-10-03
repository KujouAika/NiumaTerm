//! What the pointer is doing on the canvas, as plain transitions.
//!
//! A press on the background pans, and counts as a click when it barely
//! moved. A press on a card moves it only once the pointer has moved a
//! few pixels, so clicking a card selects it without nudging it. A press on
//! a port draws a connection to wherever the pointer is released.

use gpui::{Pixels, Point};

use crate::agent_tab::orchestration::canvas::geometry::distance;

/// How far the pointer must move before a press becomes a drag.
pub(super) const DRAG_THRESHOLD: f32 = 4.;

/// What a press started on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Background,
    Card(usize),
    Port(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Gesture {
    #[default]
    Idle,
    Panning {
        start: Point<Pixels>,
        last: Point<Pixels>,
        moved: bool,
    },
    Pressing {
        node: usize,
        start: Point<Pixels>,
    },
    Dragging {
        node: usize,
        start: Point<Pixels>,
        now: Point<Pixels>,
    },
    Connecting {
        from: usize,
        now: Point<Pixels>,
    },
}

/// What a pointer move asks the canvas to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Moved {
    Nothing,
    Pan(Point<Pixels>),
    /// A dragged card or a connection line follows the pointer.
    Redraw,
}

/// What releasing the button completed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Released {
    Nothing,
    BackgroundClick(Point<Pixels>),
    CardClick(usize),
    /// A card was dragged by this screen distance.
    Dragged {
        node: usize,
        by: Point<Pixels>,
    },
    /// `to` now depends on `from`.
    Connected {
        from: usize,
        to: usize,
    },
}

impl Gesture {
    /// The primary button went down on `target` at `at`. A canvas that
    /// cannot be edited only pans.
    pub(super) fn press(target: Target, at: Point<Pixels>, editable: bool) -> Self {
        match target {
            Target::Background => Self::Panning {
                start: at,
                last: at,
                moved: false,
            },
            Target::Card(node) if editable => Self::Pressing { node, start: at },
            Target::Port(from) if editable => Self::Connecting { from, now: at },
            Target::Card(_) | Target::Port(_) => Self::Idle,
        }
    }

    /// The pointer moved to `at` with the primary button `held` or not. A
    /// move without the button ends a gesture whose release happened
    /// outside the canvas, completing nothing.
    pub(super) fn moved(&mut self, at: Point<Pixels>, held: bool) -> Moved {
        if !held {
            let active = *self != Self::Idle;

            *self = Self::Idle;

            return if active {
                Moved::Redraw
            } else {
                Moved::Nothing
            };
        }

        match *self {
            Self::Idle => Moved::Nothing,
            Self::Panning { start, last, moved } => {
                *self = Self::Panning {
                    start,
                    last: at,
                    moved: moved || distance(at, start) >= DRAG_THRESHOLD,
                };

                Moved::Pan(at - last)
            }
            Self::Pressing { node, start } => {
                if distance(at, start) < DRAG_THRESHOLD {
                    return Moved::Nothing;
                }

                *self = Self::Dragging {
                    node,
                    start,
                    now: at,
                };

                Moved::Redraw
            }
            Self::Dragging { node, start, .. } => {
                *self = Self::Dragging {
                    node,
                    start,
                    now: at,
                };

                Moved::Redraw
            }
            Self::Connecting { from, .. } => {
                *self = Self::Connecting { from, now: at };

                Moved::Redraw
            }
        }
    }

    /// The button came up at `at`, over card `over` if any.
    pub(super) fn release(&mut self, at: Point<Pixels>, over: Option<usize>) -> Released {
        let released = match *self {
            Self::Idle => Released::Nothing,
            Self::Panning { moved: false, .. } => Released::BackgroundClick(at),
            Self::Panning { .. } => Released::Nothing,
            Self::Pressing { node, .. } => Released::CardClick(node),
            Self::Dragging { node, start, .. } => Released::Dragged {
                node,
                by: at - start,
            },
            Self::Connecting { from, .. } => match over {
                Some(to) if to != from => Released::Connected { from, to },
                _ => Released::Nothing,
            },
        };

        *self = Self::Idle;

        released
    }

    /// How far `node` is being dragged right now, in screen pixels.
    pub(super) fn drag_offset(&self, node: usize) -> Option<Point<Pixels>> {
        match *self {
            Self::Dragging {
                node: dragged,
                start,
                now,
            } if dragged == node => Some(now - start),
            _ => None,
        }
    }
}
