use crate::tabs::*;

/// Build a manager of fake surfaces (u32) with sequential ids 1..=n, ids
/// equal to the surface value for easy assertions.
fn manager(n: u32) -> TabManager<u32> {
    let mut mgr = TabManager::new(1, TabId(1), "PowerShell".into());

    for i in 2..=n {
        mgr.new_tab(i, TabId(i as u64), "PowerShell".into());
    }

    mgr
}

#[test]
fn close_is_refused_for_single_tab() {
    let mut mgr = manager(1);

    assert!(mgr.close(TabId(1)).is_none());
    assert_eq!(mgr.list().len(), 1);
}

#[test]
fn close_active_falls_to_right_neighbor() {
    let mut mgr = manager(3); // active = tab3 (index 2)

    mgr.list_mut().activate(1); // active = tab2 (index 1)

    let removed = mgr.close(TabId(2));

    assert_eq!(removed, Some(2));

    // tab3 was to the right; it is now active at index 1.
    assert_eq!(mgr.list().active_id(), TabId(3));
    assert_eq!(mgr.list().active_index(), 1);
}

#[test]
fn close_active_with_no_right_neighbor_falls_left() {
    let mut mgr = manager(3); // active = tab3 (index 2, rightmost)

    let removed = mgr.close(TabId(3));

    assert_eq!(removed, Some(3));
    assert_eq!(mgr.list().active_id(), TabId(2));
    assert_eq!(mgr.list().active_index(), 1);
}

#[test]
fn a_failure_survives_the_successes_that_follow_it() {
    let mut mgr = manager(2); // tab 2 is active

    mgr.record_outcome(TabId(1), Some(1).into());

    mgr.record_outcome(TabId(1), Some(0).into());

    assert_eq!(
        mgr.list().items()[0].last_outcome(),
        Some(CommandOutcome::Failed)
    );

    // Clearing acts on the active tab, so the flagged one keeps its result
    // until the user goes there.
    assert!(!mgr.clear_active_outcome());

    mgr.list_mut().activate(0);

    assert!(mgr.clear_active_outcome());
    assert_eq!(mgr.list().items()[0].last_outcome(), None);
    assert!(!mgr.clear_active_outcome());
}

#[test]
fn progress_state_zero_clears_the_bar() {
    let mut mgr = manager(1);

    let set = ProgressReport {
        state: ProgressState::Set,
        progress: Some(42),
    };

    mgr.set_progress(TabId(1), set);

    assert_eq!(mgr.list().items()[0].progress(), Some(set));

    mgr.set_progress(
        TabId(1),
        ProgressReport {
            state: ProgressState::Remove,
            progress: None,
        },
    );

    assert_eq!(mgr.list().items()[0].progress(), None);

    mgr.set_progress(TabId(1), set);

    mgr.clear_progress(TabId(1));

    assert_eq!(mgr.list().items()[0].progress(), None);
}

#[test]
fn id_is_stable_across_close_and_reorder() {
    let mut mgr = manager(3);

    mgr.close(TabId(1)); // indices shift, ids do not

    assert_eq!(mgr.list().items()[0].id(), TabId(2));

    mgr.list_mut().reorder(0, 1);

    assert_eq!(mgr.list().items()[1].id(), TabId(2));

    // tab3 keeps its id throughout.
    assert!(mgr.list().items().iter().any(|t| t.id() == TabId(3)));
}
