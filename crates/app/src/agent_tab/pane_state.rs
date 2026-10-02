//! Repaint scheduling for the response-age label.

use std::time::Duration;

use gpui::{Context, Task};

use crate::agent_tab::AgentPane;
use crate::agent_tab::transcript::LAST_RESPONSE_LIMIT;

#[derive(Default)]
pub(super) struct TurnPresentation {
    timer: Option<Task<()>>,
}

impl TurnPresentation {
    pub(super) fn refresh_timer(&mut self, cx: &mut Context<AgentPane>) {
        if self.timer.is_some() {
            return;
        }

        self.timer = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(interval) = this.update(cx, |this, cx| {
                    cx.notify();

                    this.session
                        .borrow()
                        .conversation()
                        .borrow()
                        .last_response_at
                        .and_then(|at| response_age_tick(at.elapsed()))
                }) else {
                    return;
                };

                let Some(interval) = interval else {
                    let _ = this.update(cx, |this, _| this.turn.timer = None);

                    return;
                };

                cx.background_executor().timer(interval).await;
            }
        }));

        cx.notify();
    }
}

/// How long the response-age label can go without a repaint: it counts
/// seconds for the first minute and minutes after that, and stops changing
/// once the age passes the label's limit.
fn response_age_tick(age: Duration) -> Option<Duration> {
    const MINUTE: u64 = 60;

    match age.as_secs() {
        ..MINUTE => Some(Duration::from_secs(1)),
        seconds if seconds < LAST_RESPONSE_LIMIT.as_secs() => Some(Duration::from_secs(MINUTE)),
        _ => None,
    }
}
