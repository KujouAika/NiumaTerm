//! Transcript hooks with no clocks, counters, or reporting task.

#[cfg(test)]
#[path = "disabled_tests.rs"]
mod tests;

use crate::transcript::Operation;

/// Builds without profiling hold no measurement state.
#[must_use]
pub enum Probe {}

impl Probe {
    /// Keep update call sites identical without opening a measurement scope.
    #[inline(always)]
    pub fn start(_: Operation) -> Option<Self> {
        None
    }
}

/// No samples are retained without performance collection.
#[inline(always)]
pub fn flush() {}
