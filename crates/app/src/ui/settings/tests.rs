use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
use std::{fs, io};

use app::agent_tab::AgentKind;
use app::terminal_tab::settings::TerminalSettings;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ListAlignment,
    ListOffset, ListState, ParentElement as _, ScrollDelta, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled as _, Task, TestAppContext, Window, div, list, point,
    px, size,
};
use gpui_component::Root;
use gpui_component::setting::{SelectIndex, SettingsState};
use nmt_config::Config;
use nmt_config::appearance::SmoothScrollingMode;
use nmt_config::builtin_themes::THEMES as BUILTIN_THEMES;
use nmt_config::theme_catalog::theme_families;

use crate::ui::settings::state::{
    AgentProfile, AgentProfileLauncher, AppSettings, DEFAULT_FONT_FAMILY, DEFAULT_FONT_SIZE,
    EnvVar, SettingsEditing, agent_kind_display_label, builtin_agent_profile,
};
use crate::ui::settings::terminal_bridge::install_terminal_settings;
use crate::ui::settings::{OpenSettings, SettingsSurface, new_settings_view, save_settings_to};

struct SettingsHost(SettingsSurface);

impl gpui::Render for SettingsHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().children(self.0.render(cx))
    }
}

#[gpui::test]
fn closed_settings_release_local_edits_while_another_window_stays_open(cx: &mut TestAppContext) {
    use gpui::VisualTestContext;

    cx.update(|cx| {
        gpui_component::init(cx);

        cx.set_global(AppSettings::default());
    });

    let mut windows = Vec::new();
    let mut editors = Vec::new();

    for _ in 0..2 {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let editing = cx.new(|_| SettingsEditing::default());

                editors.push(editing.downgrade());

                let state = SettingsState::owned(
                    SelectIndex {
                        page_ix: 0,
                        group_ix: Some(1),
                    },
                    window,
                    cx,
                );

                let view = new_settings_view(state, editing, cx);

                cx.new(|_| {
                    SettingsHost(SettingsSurface {
                        open: Some(OpenSettings {
                            view,
                            _theme_watcher: None,
                            _pairing_renewal: Task::ready(()),
                        }),
                    })
                })
            })
            .unwrap()
        });

        windows.push(window);
    }

    cx.run_until_parked();

    editors[0]
        .update(cx, |editing, cx| {
            editing.theme_filter = "First window".into();

            cx.notify();
        })
        .unwrap();

    cx.update(|cx| {
        assert!(
            editors[1]
                .upgrade()
                .unwrap()
                .read(cx)
                .theme_filter
                .is_empty()
        );
    });

    let mut cx = VisualTestContext::from_window(windows[0].into(), cx);

    windows[0]
        .update(&mut cx, |host, _, cx| {
            host.0.retire();

            cx.notify();
        })
        .unwrap();

    // Render state keeps the previous frame alive until the next frame
    // completes, so both retained frames must stop referencing the page.
    for _ in 0..2 {
        windows[0].update(&mut cx, |_, _, cx| cx.notify()).unwrap();

        cx.run_until_parked();

        cx.refresh().unwrap();

        cx.run_until_parked();
    }

    assert!(editors[0].upgrade().is_none());
    assert!(editors[1].upgrade().is_some());
    assert!(
        editors[0]
            .update(&mut cx, |editing, _| {
                editing.theme_filter = "Late completion".into();
            })
            .is_err()
    );
}

#[gpui::test]
fn powershell_compatibility_changes_reach_the_live_terminal_snapshot(cx: &mut TestAppContext) {
    cx.set_global(AppSettings::default());

    cx.update(install_terminal_settings);

    assert!(cx.read(|cx| {
        cx.global::<TerminalSettings>()
            .improve_powershell_compatibility
    }));

    cx.update_global::<AppSettings, _>(|settings, _| {
        settings.edit_terminal(|section| section.improve_powershell_compatibility = false);
    });

    assert!(!cx.read(|cx| {
        cx.global::<TerminalSettings>()
            .improve_powershell_compatibility
    }));
}

struct ThemeGalleryProbe {
    editing: Entity<SettingsEditing>,
    width: gpui::Pixels,
    scroll: gpui::ScrollHandle,
    _updates: gpui::Subscription,
}

impl gpui::Render for ThemeGalleryProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::ui::settings::theme_gallery::theme_list;

        div()
            .id("theme-gallery-probe")
            .size_full()
            .relative()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .w(self.width)
                    .child(theme_list(self.editing.clone(), cx)),
            )
    }
}

#[gpui::test]
fn theme_grid_measures_its_own_width_inside_a_wider_settings_page(cx: &mut TestAppContext) {
    use gpui::VisualTestContext;

    cx.update(gpui_component::init);
    cx.set_global(AppSettings::default());

    let handle = cx.add_window(|_, cx| {
        let editing = cx.new(|_| SettingsEditing {
            theme_families: theme_families(
                BUILTIN_THEMES
                    .iter()
                    .map(|builtin| {
                        (
                            builtin.name.to_owned(),
                            toml::from_str(builtin.source).unwrap(),
                        )
                    })
                    .collect(),
            )
            .into(),
            ..SettingsEditing::default()
        });

        let updates = cx.observe(&editing, |_, _, cx| cx.notify());

        ThemeGalleryProbe {
            editing,
            width: px(650.),
            scroll: gpui::ScrollHandle::default(),
            _updates: updates,
        }
    });

    let mut cx = VisualTestContext::from_window(handle.into(), cx);

    cx.simulate_resize(size(px(1400.), px(900.)));

    for (width, columns) in [(650., 3), (396., 1), (900., 4)] {
        cx.update(|window, cx| {
            window
                .root::<ThemeGalleryProbe>()
                .flatten()
                .unwrap()
                .update(cx, |probe, cx| {
                    probe.width = px(width);

                    cx.notify();
                });
        });

        // Layout measures the grid, the next frame applies its column count,
        // and the final frame renders the new rows without keyboard input.
        for _ in 0..3 {
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });

            cx.run_until_parked();
        }

        cx.update(|window, cx| {
            let probe = window.root::<ThemeGalleryProbe>().flatten().unwrap();

            assert_eq!(probe.read(cx).editing.read(cx).theme_columns, columns);
        });
    }
}

#[gpui::test]
fn theme_grid_only_builds_visible_cards_and_keeps_scrolled_cards_selectable(
    cx: &mut TestAppContext,
) {
    use gpui::{Modifiers, VisualTestContext};
    use nmt_config::theme::AppearanceTheme;

    cx.update(gpui_component::init);
    cx.set_global(AppSettings::default());
    cx.update(|cx| cx.set_smooth_wheel_scrolling(false));

    let handle = cx.add_window(|_, cx| {
        let editing = cx.new(|_| SettingsEditing {
            theme_families: theme_families(
                BUILTIN_THEMES
                    .iter()
                    .map(|builtin| {
                        (
                            builtin.name.to_owned(),
                            toml::from_str(builtin.source).unwrap(),
                        )
                    })
                    .collect(),
            )
            .into(),
            ..Default::default()
        });

        ThemeGalleryProbe {
            _updates: cx.observe(&editing, |_, _, cx| cx.notify()),
            editing,
            width: px(396.),
            scroll: gpui::ScrollHandle::default(),
        }
    });

    let mut cx = VisualTestContext::from_window(handle.into(), cx);

    cx.simulate_resize(size(px(800.), px(220.)));

    for _ in 0..3 {
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
    }

    assert!(cx.debug_bounds("theme-card-0").is_some());
    assert!(cx.debug_bounds("theme-card-6").is_none());

    cx.simulate_event(ScrollWheelEvent {
        position: point(px(200.), px(100.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-1000.))),
        ..Default::default()
    });

    assert!(cx.debug_bounds("theme-card-0").is_none());

    let last_card = cx
        .debug_bounds("theme-card-6")
        .expect("the last row must become visible");

    let (editing, expected) = cx.update(|window, cx| {
        let editing = window
            .root::<ThemeGalleryProbe>()
            .flatten()
            .unwrap()
            .read(cx)
            .editing
            .clone();

        let expected = editing.read(cx).theme_families[6]
            .variant(AppearanceTheme::Dark)
            .id
            .clone();

        (editing, expected)
    });

    cx.simulate_click(last_card.center(), Modifiers::default());
    cx.update(|_, cx| assert_eq!(cx.global::<AppSettings>().config().theme, expected));

    cx.update(|_, cx| {
        editing.update(cx, |editing, cx| {
            editing.theme_filter = expected;

            cx.notify();
        })
    });

    assert!(
        cx.debug_bounds("theme-card-6").is_some(),
        "filtering must retain the matching card"
    );
    assert!(cx.debug_bounds("theme-card-0").is_none());
}

#[gpui::test]
fn paired_theme_switch_preserves_geometry_and_survives_config_reload(cx: &mut TestAppContext) {
    use crate::ui::settings::theme::select_theme;
    use app::design::{CARD_RADIUS, CONTROL_RADIUS};
    use gpui_component::ActiveTheme as _;
    use nmt_config::theme::AppearanceTheme;

    cx.update(gpui_component::init);
    cx.set_global(AppSettings::default());

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");

    for (id, mode) in [
        ("claude_light", AppearanceTheme::Light),
        ("claude_dark", AppearanceTheme::Dark),
    ] {
        cx.update(|cx| {
            assert!(select_theme(id.into(), Config::load_named_theme(id), cx));
            assert_eq!(cx.theme().mode.is_dark(), mode == AppearanceTheme::Dark);
            assert_eq!(cx.theme().radius, CONTROL_RADIUS);
            assert_eq!(cx.theme().radius_lg, CARD_RADIUS);

            fs::write(
                &path,
                toml::to_string(cx.global::<AppSettings>().config()).unwrap(),
            )
            .unwrap();

            let restored = Config::load_for_startup_from(&path, directory.path()).unwrap();

            assert_eq!(restored.theme, id);
            assert_eq!(restored.ui_theme.unwrap().mode, mode);
        });
    }

    cx.update(|cx| {
        let background = cx.theme().background;

        assert!(!select_theme(
            "../missing-theme".into(),
            Err("missing theme".into()),
            cx
        ));
        assert_eq!(cx.global::<AppSettings>().config().theme, "claude_dark");
        assert_eq!(cx.theme().background, background);
    });
}

#[test]
fn profile_mutations_keep_default_valid() {
    let mut settings = AppSettings::default();

    // Add: unique placeholder names.
    settings.add_profile();

    settings.add_profile();

    assert_eq!(settings.config().profiles.list.len(), 3);
    assert_eq!(settings.config().profiles.list[1].name, "Profile 2");
    assert_eq!(settings.config().profiles.list[2].name, "Profile 3");

    // Rename the default: the reference follows.
    settings.rename_profile(0, "Pwsh".into());

    assert_eq!(settings.config().profiles.default, "Pwsh");

    // Remove the default: falls back to the first remaining profile.
    settings.remove_profile(0);

    assert_eq!(settings.config().profiles.default, "Profile 2");

    // The last profile cannot be removed.
    settings.remove_profile(0);

    settings.remove_profile(0);

    assert_eq!(settings.config().profiles.list.len(), 1);
}

#[test]
fn agent_profile_mutations_keep_default_valid() {
    let mut settings = AppSettings::default();

    // One seeded profile per registered harness, the first of which is the
    // default a new installation launches.
    assert_eq!(
        settings.config().agent_profiles.list.len(),
        AgentKind::ALL.len()
    );
    assert_eq!(settings.config().agent_profiles.default, "Claude Code");

    // Unique-name resolution: an empty desired name takes the kind
    // label, collisions get a numeric suffix, and the excluded index
    // (edit mode) keeps its own name available.
    assert_eq!(
        settings.unique_agent_profile_name("", AgentKind::Claude, None),
        "Claude Code 2"
    );
    assert_eq!(
        settings.unique_agent_profile_name("Codex", AgentKind::Codex, Some(1)),
        "Codex"
    );
    assert_eq!(
        settings.unique_agent_profile_name(" Mine ", AgentKind::Codex, None),
        "Mine"
    );

    // Update with a rename: the default reference follows.
    let renamed = AgentProfile {
        name: "Proxy".into(),
        ..settings.config().agent_profiles.list[0].clone()
    };

    settings.save_agent_profile(Some(0), renamed);

    assert_eq!(settings.config().agent_profiles.default, "Proxy");

    // Remove the default: falls back to the first remaining profile.
    settings.remove_agent_profile(0);

    assert_eq!(settings.config().agent_profiles.default, "Codex");

    // Every profile can be removed; an empty list clears the default.
    while !settings.config().agent_profiles.list.is_empty() {
        settings.remove_agent_profile(0);
    }

    assert!(settings.config().agent_profiles.list.is_empty());
    assert_eq!(settings.config().agent_profiles.default, "");

    // The shortcut fallback still produces a launchable profile.
    assert_eq!(
        settings.default_agent_profile_entry().kind,
        AgentKind::Claude
    );
}

#[test]
fn live_settings_edits_normalize_values_and_preserve_other_configuration() {
    let mut settings = AppSettings::from_config(Config {
        working_dir: Some("retained-directory".into()),
        ..Config::default()
    });

    settings.edit_appearance(|appearance| {
        appearance.terminal_font_family = "   ".into();
        appearance.terminal_font_size = f64::NAN;
        appearance.background_opacity = -1.0;
    });

    assert_eq!(
        settings.config().working_dir.as_deref(),
        Some("retained-directory")
    );
    assert_eq!(
        settings.config().appearance.terminal_font_family,
        DEFAULT_FONT_FAMILY
    );
    assert_eq!(
        settings.config().appearance.terminal_font_size,
        DEFAULT_FONT_SIZE
    );
    assert_eq!(settings.config().appearance.background_opacity, 0.2);
}

#[test]
fn profile_edits_keep_names_and_defaults_valid_across_reordering() {
    let mut settings = AppSettings::default();

    assert!(settings.duplicate_agent_profile(0));
    assert_eq!(
        settings.config().agent_profiles.list[1].name,
        "Claude Code 2"
    );
    assert!(settings.move_agent_profile(0, 2));
    assert_eq!(settings.config().agent_profiles.default, "Claude Code");
    assert_eq!(settings.default_agent_profile_entry().name, "Claude Code");

    let previous = settings.config().clone();

    assert!(!settings.move_agent_profile(99, 0));
    assert!(!settings.duplicate_agent_profile(99));
    assert!(!settings.save_agent_profile(Some(99), AgentProfile::default()));
    assert!(!settings.set_profile_shell(99, "missing".into()));
    assert_eq!(settings.config(), &previous);

    while !settings.config().agent_profiles.list.is_empty() {
        settings.remove_agent_profile(0);
    }

    assert!(settings.save_agent_profile(
        None,
        AgentProfile {
            name: "  Replacement  ".into(),
            env: vec![EnvVar {
                name: " ".into(),
                value: "unused".into()
            }],
            ..AgentProfile::default()
        }
    ));
    assert_eq!(settings.config().agent_profiles.default, "Replacement");
    assert!(settings.default_agent_profile_entry().env.is_empty());
}

#[test]
fn failed_settings_save_keeps_edits_for_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    fs::write(&path, "invalid [ configuration").unwrap();

    let mut settings = AppSettings::default();

    assert!(settings.config().terminal.improve_powershell_compatibility);

    settings.edit_terminal(|section| section.improve_powershell_compatibility = false);

    settings.edit_appearance(|section| section.scroll_to_bottom_when_typing = false);

    settings.edit_appearance(|section| section.reduce_motion = true);

    settings.edit_appearance(|section| section.human_friendly_agent_ui_layout = false);

    settings.edit_appearance(|section| section.smooth_scrolling = SmoothScrollingMode::OnlyAgent);

    let error = settings.save_to(&path).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(settings.config().appearance.reduce_motion);
    assert!(!settings.config().appearance.scroll_to_bottom_when_typing);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "invalid [ configuration"
    );

    fs::write(&path, "# keep this\n[appearance]\nfuture-setting = 42\n").unwrap();

    settings.save_to(&path).unwrap();

    let saved = fs::read_to_string(&path).unwrap();

    assert!(saved.contains("# keep this"));
    assert!(saved.contains("future-setting = 42"));

    let config: Config = toml::from_str(&saved).unwrap();

    assert_eq!(config.appearance, settings.config().appearance);
    assert_eq!(config.agent, settings.config().agent);
    assert_eq!(config.system, settings.config().system);
    assert_eq!(config.update, settings.config().update);
    assert!(!config.terminal.improve_powershell_compatibility);
}

#[test]
fn settings_io_failure_preserves_edits_until_the_path_is_repaired() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    fs::create_dir(&path).unwrap();

    let mut settings = AppSettings::default();

    settings.edit_system(|section| section.confirm_before_closing_workspace = false);

    assert!(settings.save_to(&path).is_err());
    assert!(!settings.config().system.confirm_before_closing_workspace);
    assert!(path.is_dir());

    fs::remove_dir(&path).unwrap();

    settings.save_to(&path).unwrap();

    let config: Config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();

    assert!(!config.system.confirm_before_closing_workspace);
}

fn list_pixel_position(state: &ListState) -> f32 {
    let offset = state.logical_scroll_top();

    offset.item_ix as f32 * 20. + offset.offset_in_item.as_f32()
}

struct SettingsAwareList(ListState);

impl gpui::Render for SettingsAwareList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.set_smooth_wheel_enabled(
            cx.global::<AppSettings>()
                .config()
                .appearance
                .smooth_scrolling
                .terminal_enabled(),
        );

        list(self.0.clone(), |_, _, _| {
            div().h(px(20.)).w_full().into_any_element()
        })
        .w_full()
        .h_full()
    }
}

fn draw_settings_aware_list(cx: &mut gpui::VisualTestContext, view: &Entity<SettingsAwareList>) {
    cx.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
        view.clone().into_any_element()
    });
}

#[gpui::test]
fn smooth_scrolling_mode_updates_an_open_terminal_list(cx: &mut TestAppContext) {
    cx.set_global(AppSettings::default());

    let state = ListState::new(50, ListAlignment::Top, px(10.)).measure_all();

    state.scroll_to(ListOffset {
        item_ix: 10,
        offset_in_item: px(0.),
    });

    let cx = cx.add_empty_window();
    let view = cx.update(|_, cx| cx.new(|_| SettingsAwareList(state.clone())));

    draw_settings_aware_list(cx, &view);

    cx.simulate_event(ScrollWheelEvent {
        position: point(px(1.), px(1.)),
        delta: ScrollDelta::Lines(point(0., 1.)),
        ..Default::default()
    });

    assert_eq!(list_pixel_position(&state), 200.);

    cx.executor().advance_clock(Duration::from_millis(100));

    draw_settings_aware_list(cx, &view);

    let stopped_at = list_pixel_position(&state);

    assert!(stopped_at > 150. && stopped_at < 200.);

    cx.update_global::<AppSettings, _>(|settings, _| {
        settings
            .edit_appearance(|section| section.smooth_scrolling = SmoothScrollingMode::OnlyAgent);
    });

    draw_settings_aware_list(cx, &view);

    cx.executor().advance_clock(Duration::from_millis(400));

    draw_settings_aware_list(cx, &view);

    assert!((list_pixel_position(&state) - stopped_at).abs() < 0.1);
}

#[test]
fn every_registered_harness_can_be_named_seeded_and_launched() {
    // A kind that is selectable in one surface and missing from another is
    // invisible in practice: the add dialog's picker, the seeded list, and the
    // built-in profile all have to agree on the same registry.
    for kind in AgentKind::ALL {
        let profile = builtin_agent_profile(kind);
        let id: &str = kind.into();

        assert_eq!(profile.kind, kind, "{}", id);
        assert!(!profile.executable.trim().is_empty(), "{}", id);
        assert!(!profile.name.trim().is_empty(), "{}", id);
        assert!(
            !agent_kind_display_label(kind).is_empty(),
            "{} has no display label",
            id
        );
    }

    // Round-tripping catches a conversion that quietly maps a new kind onto an
    // existing one, which would make its profiles open the wrong backend.
    for kind in AgentKind::ALL {
        assert_eq!(AgentKind::from_id(kind.into()), Some(kind));
    }

    assert_eq!(
        builtin_agent_profile(AgentKind::DeepSeek).launcher,
        AgentProfileLauncher::Npx
    );
}

#[gpui::test]
fn windows_notification_switch_restores_imported_disabled_setting(cx: &mut TestAppContext) {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_component::setting::AnySettingField;

    use crate::ui::settings::system_page::windows_notification_field;

    let mut settings = AppSettings::default();

    settings.edit_system(|section| section.send_system_notifications = false);
    cx.set_global(settings);

    let registered = Rc::new(Cell::new(true));

    let field = windows_notification_field(
        {
            let registered = registered.clone();

            move || registered.get()
        },
        {
            let registered = registered.clone();

            move |enabled| {
                registered.set(enabled);

                Ok(())
            }
        },
    )
    .default_value(true);

    let cx = cx.add_empty_window();

    cx.update(|window, cx| {
        // Reset uses the same setter as clicking the switch, and dirty state
        // reads its displayed value through the same getter.
        assert!(field.is_resettable(cx));

        field.reset(window, cx);

        assert!(!field.is_resettable(cx));
        assert!(
            cx.global::<AppSettings>()
                .config()
                .system
                .send_system_notifications
        );
        assert!(registered.get());

        let field = field.default_value(false);

        assert!(field.is_resettable(cx));

        field.reset(window, cx);

        assert!(!field.is_resettable(cx));
        assert!(
            !cx.global::<AppSettings>()
                .config()
                .system
                .send_system_notifications
        );
        assert!(!registered.get());
    });
}

#[gpui::test]
fn windows_notification_switch_keeps_setting_after_registration_failure(cx: &mut TestAppContext) {
    use anyhow::anyhow;
    use gpui_component::setting::AnySettingField;

    use crate::ui::settings::system_page::windows_notification_field;

    let mut settings = AppSettings::default();

    settings.edit_system(|section| section.send_system_notifications = false);
    cx.set_global(settings);

    let field = windows_notification_field(|| false, |_| Err(anyhow!("registration failed")))
        .default_value(true);

    let cx = cx.add_empty_window();

    cx.update(|window, cx| {
        field.reset(window, cx);

        assert!(
            !cx.global::<AppSettings>()
                .config()
                .system
                .send_system_notifications
        );
        assert!(field.is_resettable(cx));
    });
}

#[gpui::test]
fn background_save_completes_only_after_edits_made_during_the_write_are_saved(
    cx: &mut TestAppContext,
) {
    use gpui::VisualTestContext;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");

    fs::write(&path, "# retained\n").unwrap();

    let completed = Rc::new(Cell::new(None));

    cx.update(|cx| {
        gpui_component::init(cx);

        cx.set_global(AppSettings::default());

        // Nothing was edited since the configuration was read, so quitting
        // must leave the file alone.
        assert!(!cx.global::<AppSettings>().should_save_on_exit());
    });

    let window = cx.add_window(|window, cx| {
        let content = cx.new(|_| SettingsHost(SettingsSurface::default()));

        Root::new(content, window, cx)
    });

    let mut cx = VisualTestContext::from_window(window.into(), cx);

    cx.update(|window, cx| {
        let completed = completed.clone();

        let saved = save_settings_to(path.clone(), window, cx);

        window
            .spawn(cx, async move |_| completed.set(Some(saved.await)))
            .detach();

        cx.global_mut::<AppSettings>()
            .edit_appearance(|appearance| appearance.reduce_motion = true);

        assert_eq!(fs::read_to_string(&path).unwrap(), "# retained\n");
    });

    assert_eq!(completed.get(), None);

    cx.run_until_parked();

    assert_eq!(completed.get(), Some(true));

    let config: Config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();

    assert!(config.appearance.reduce_motion);

    // Both writes landed, so a quit now has nothing left to save.
    cx.update(|_, cx| assert!(!cx.global::<AppSettings>().should_save_on_exit()));

    fs::write(&path, "invalid [ configuration").unwrap();
    completed.set(None);

    cx.update(|window, cx| {
        let completed = completed.clone();

        let saved = save_settings_to(path.clone(), window, cx);

        window
            .spawn(cx, async move |_| completed.set(Some(saved.await)))
            .detach();
    });

    cx.run_until_parked();

    assert_eq!(completed.get(), Some(false));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "invalid [ configuration"
    );
}
