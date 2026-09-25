mod detail_navigation {
    use nmt_agent::background_task::BackgroundTaskKey;

    use crate::ui::background_tasks::PanelMode;

    #[test]
    fn opening_a_child_replaces_the_list_and_returning_restores_it() {
        let mut mode = PanelMode::List;

        assert_eq!(mode.detail_key(), None);
        assert_eq!(mode.close(), None, "the list is already showing");

        let key = BackgroundTaskKey::codex("thr_child");

        mode.open(key.clone(), true, false);

        assert_eq!(mode.detail_key(), Some(&key));

        // One view at a time: opening a child is not a second column.
        assert_eq!(
            mode.close(),
            Some((true, false)),
            "returning restores the sections the user had open"
        );
        assert_eq!(mode.detail_key(), None);
    }

    #[test]
    fn each_child_is_opened_in_its_own_right() {
        let mut mode = PanelMode::List;

        mode.open(BackgroundTaskKey::codex("a"), false, false);

        mode.open(BackgroundTaskKey::claude_code("a"), false, true);

        // Same local id, different providers: the qualified key keeps them apart.
        assert_eq!(
            mode.detail_key(),
            Some(&BackgroundTaskKey::claude_code("a"))
        );
        assert_eq!(mode.close(), Some((false, true)));
    }
}
