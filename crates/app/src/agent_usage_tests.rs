use std::sync::Arc;

use futures::executor::block_on;
use futures::future::BoxFuture;
use nmt_agent::usage::FetchCancellation;

use crate::agent_usage::*;
use crate::usage_refresh::FetchError;

#[gpui::test]
fn changing_usage_launcher_discards_the_previous_request(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;

    cx.update(|cx| {
        let mut settings = AppSettings::default();

        settings.edit_agent(|section| section.show_agent_usage = false);

        cx.set_global(settings);
    });

    let view = cx.new(|_| AgentUsageView {
        providers: [0, 1].map(|_| {
            Refresh::new(
                UsageSnapshot {
                    updated_at: Some(123),
                    ..UsageSnapshot::default()
                },
                Arc::new(|_: Arc<FetchCancellation>| -> BoxFuture<'static, Result<UsageSnapshot, FetchError>> {
                    panic!("cancelled source must not run")
                }),
                true,
            )
        }),
        enabled: true,
        codex_launcher: AgentCli::new("old-codex", []),
    });

    let old = view.update(cx, |view, _| view.providers[0].begin().unwrap());

    view.update(cx, |view, cx| view.on_settings_changed(cx));

    let fetched = block_on(old.run());

    view.update(cx, |view, _| {
        assert!(matches!(
            view.providers[0].complete(fetched),
            Completion::Discarded
        ));
        assert!(view.providers[0].value.updated_at.is_none());
        assert!(!view.providers[0].refreshing());
        assert_eq!(view.codex_launcher.executable(), "codex");
    });
}
