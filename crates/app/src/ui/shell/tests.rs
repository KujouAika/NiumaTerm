use std::cell::Cell;
use std::rc::Rc;

use app::agent_tab::AgentKind;
use app::agent_tab::settings::AgentSettings;
use app::agent_tab::team::{TeamPane, TeamRuntime};
use gpui::{AnyWindowHandle, AppContext as _, EmptyView, TestAppContext, WeakEntity};
use gpui_component::input::InputState;
use nmt_agent::AgentWorkspace;
use nmt_agent::team::session::TeamSession;
use nmt_config::local_state::{
    SessionState, TabFold, TabState, WindowLocalState, WindowState, WorkspaceState,
};
use tempfile::tempdir;

use crate::ui::shell::close_confirm::should_confirm_close;
use crate::ui::shell::{InlineRename, InlineRenameStyle, TabSurface, WindowRegistry};

struct InlineRenameProbe {
    input: gpui::Entity<InputState>,
    cancelled: Rc<Cell<bool>>,
}

impl gpui::Render for InlineRenameProbe {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let cancelled = self.cancelled.clone();

        InlineRename::new(
            "inline-rename-probe",
            "Rename probe",
            self.input.clone(),
            InlineRenameStyle::HorizontalTab,
            move |_, _| cancelled.set(true),
        )
    }
}

#[gpui::test]
fn restored_agent_tab_keeps_kind_before_activation(cx: &mut TestAppContext) {
    let surface = TabSurface::Pending(Box::new(TabState {
        agent: Some("codex".to_string()),
        ..TabState::default()
    }));

    assert!(cx.update(|cx| surface.agent_kind(cx)) == Some(AgentKind::Codex));
}

#[gpui::test]
fn disabling_agent_team_releases_the_runtime_and_keeps_the_saved_room(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();

    cx.update(|cx| {
        gpui_component::init(cx);

        cx.set_global(AgentSettings::default());
    });

    let cx = cx.add_empty_window();

    let (runtime, room_id) = cx.update(|window, cx| {
        let runtime = TeamRuntime::create(directory.path(), AgentWorkspace::default(), cx);
        let room_id = runtime.read(cx).room().id();
        let weak = runtime.downgrade();
        let pane = cx.new(|cx| TeamPane::new(runtime, window, cx));

        let mut surface = TabSurface::Team(pane);

        assert!(surface.disable_team(cx));

        let TabSurface::TeamDisabled(saved) = surface else {
            panic!("disabled team retained its live pane");
        };

        assert_eq!(saved.team_room, Some(room_id.to_string()));

        (weak, room_id)
    });

    cx.run_until_parked();

    assert!(runtime.upgrade().is_none());

    cx.update(|_, _| {});
    cx.run_until_parked();

    let restored = TeamSession::open(directory.path(), room_id).unwrap();

    assert_eq!(restored.store().room().id(), room_id);
}

#[test]
fn window_close_honors_confirmation_setting() {
    use nmt_config::system::WarnBeforeTerminatingShell::{
        Always, Disabled, WhenChildProcessesRunning,
    };
    use std::io;

    for mode in [Disabled, WhenChildProcessesRunning, Always] {
        assert!(should_confirm_close(true, mode, &Ok(0)));
        assert!(should_confirm_close(
            true,
            mode,
            &Err(io::Error::other("query failed"))
        ));
    }

    assert!(!should_confirm_close(false, Disabled, &Ok(2)));
    assert!(!should_confirm_close(
        false,
        Disabled,
        &Err(io::Error::other("query failed"))
    ));
    assert!(!should_confirm_close(
        false,
        WhenChildProcessesRunning,
        &Ok(0)
    ));
    assert!(should_confirm_close(
        false,
        WhenChildProcessesRunning,
        &Ok(2)
    ));
    assert!(should_confirm_close(
        false,
        WhenChildProcessesRunning,
        &Err(io::Error::other("query failed"))
    ));
    assert!(should_confirm_close(false, Always, &Ok(0)));
}

#[gpui::test]
fn inline_rename_routes_escape_to_cancellation(cx: &mut TestAppContext) {
    use gpui::{AppContext as _, VisualTestContext};

    let cancelled = Rc::new(Cell::new(false));

    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            gpui_component::init(cx);

            let input = cx.new(|cx| InputState::new(window, cx).default_value("old name"));

            input.update(cx, |input, cx| input.focus(window, cx));

            let probe = cx.new(|_| InlineRenameProbe {
                input,
                cancelled: cancelled.clone(),
            });

            cx.new(|cx| gpui_component::Root::new(probe, window, cx))
        })
        .expect("open inline rename test window")
    });

    let mut cx = VisualTestContext::from_window(window.into(), cx);

    cx.run_until_parked();

    cx.refresh().expect("render inline rename probe");

    cx.simulate_keystrokes("escape");

    cx.run_until_parked();

    assert!(cancelled.get());
}

fn window_state() -> WindowState {
    WindowState {
        x: 1.0,
        y: 2.0,
        width: 800.0,
        height: 600.0,
        maximized: false,
    }
}

fn session_state() -> SessionState {
    SessionState {
        active_workspace: 0,
        workspaces: vec![WorkspaceState {
            name: "Workspace 1".into(),
            cwd: Some("C:/Projects/example".into()),
            additional_cwds: Vec::new(),
            pinned: false,
            active_tab: 0,
            tab_fold: TabFold::All,
            tabs: vec![TabState {
                name: None,
                user_named: false,
                shell: Some("pwsh.exe".into()),
                args: vec!["-NoLogo".into()],
                cwd: Some("C:/Projects/example/repo".into()),
                agent: None,
                agent_profile: None,
                team_room: None,
                orchestration: false,
                orchestration_run: None,
                git_cwd: None,
                title: None,
                agent_conversation: None,
                remote_host: None,
                remote_session: None,
                shared_agent: None,
                shared_terminal: None,
                agent_settings: None,
                panes: None,
                grid_size: None,
            }],
        }],
    }
}

fn remembered() -> WindowLocalState {
    WindowLocalState {
        window: Some(window_state()),
        session: Some(session_state()),
        sidebar_width: Some(220.0),
    }
}

fn register_window(
    cx: &mut TestAppContext,
    registry: &mut WindowRegistry,
    state: WindowLocalState,
) -> AnyWindowHandle {
    let handle = cx.add_window(|_, _| EmptyView).into();

    registry.register(handle, WeakEntity::new_invalid(), state);

    handle
}

#[gpui::test]
fn closed_windows_leave_no_dispatch_targets_and_only_last_state_is_saved(cx: &mut TestAppContext) {
    let mut registry = WindowRegistry::default();

    let first = register_window(cx, &mut registry, remembered());
    let second = register_window(cx, &mut registry, remembered());

    registry.get_mut(second.window_id()).unwrap().sidebar_width = Some(300.0);

    assert!(registry.close(first.window_id()));
    assert!(registry.get(first.window_id()).is_none());
    assert_eq!(registry.windows().len(), 1);
    assert_eq!(
        registry
            .prioritized(Some(first.window_id()))
            .next()
            .unwrap()
            .handle,
        second
    );

    assert!(registry.close(second.window_id()));
    assert!(!registry.close(first.window_id()));
    assert!(registry.windows().is_empty());
    assert_eq!(
        registry.states().cloned().collect::<Vec<_>>(),
        vec![WindowLocalState {
            sidebar_width: Some(300.0),
            ..remembered()
        }]
    );

    cx.update(|cx| {
        cx.set_global(registry);

        assert!(!WindowRegistry::dispatch(cx, |_, _, _| {
            panic!("closed windows must not receive events");
        }));
    });
}

#[gpui::test]
fn reopening_replaces_retained_state_without_saving_a_duplicate(cx: &mut TestAppContext) {
    let mut registry = WindowRegistry::default();

    let first = register_window(cx, &mut registry, remembered());

    registry.close(first.window_id());

    let restored = registry.take_last_closed().unwrap();

    assert_eq!(restored, remembered());
    assert!(registry.take_last_closed().is_none());

    let reopened = register_window(cx, &mut registry, restored);

    assert_eq!(registry.states().count(), 1);

    registry.close(reopened.window_id());

    let fresh = WindowLocalState {
        sidebar_width: Some(350.0),
        ..WindowLocalState::default()
    };

    let replacement = register_window(cx, &mut registry, fresh.clone());

    assert!(registry.take_last_closed().is_none());
    assert_eq!(registry.windows()[0].handle, replacement);
    assert_eq!(registry.states().cloned().collect::<Vec<_>>(), vec![fresh]);
}

#[gpui::test]
fn dispatch_can_close_windows_and_stops_at_first_accepted_target(cx: &mut TestAppContext) {
    let mut registry = WindowRegistry::default();

    let windows: Vec<_> = (0..3)
        .map(|_| register_window(cx, &mut registry, remembered()))
        .collect();

    assert_eq!(
        registry
            .prioritized(Some(windows[0].window_id()))
            .map(|entry| entry.handle)
            .collect::<Vec<_>>(),
        vec![windows[0], windows[2], windows[1]]
    );

    cx.update(|cx| {
        cx.set_global(registry);

        let mut visited = Vec::new();

        let accepted = WindowRegistry::dispatch(cx, |handle, _, cx| {
            visited.push(handle);

            cx.global_mut::<WindowRegistry>().close(handle.window_id());

            handle == windows[1]
        });

        assert!(accepted);
        assert_eq!(visited, windows[..2]);
        assert_eq!(
            cx.global::<WindowRegistry>().windows()[0].handle,
            windows[2]
        );
        assert_eq!(cx.global::<WindowRegistry>().states().count(), 1);
    });
}
