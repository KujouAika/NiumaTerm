use std::fs;
use std::path::PathBuf;

use gpui::{
    AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds,
    WindowOptions, point, px, size,
};
use gpui_component::Root;
use nmt_agent::orchestration::canonical::to_canonical_json;
use nmt_agent::orchestration::definition::{Definition, Position};
use nmt_agent::orchestration::edit::Edit;
use tempfile::{TempDir, tempdir};

use crate::agent_tab::orchestration::editor::{Conflict, DefinitionEditor};
use crate::agent_tab::settings::AgentSettings;

const HAND_WRITTEN: &str = r#"{ "version": 1,
  "slots": { "dev": { "profile": { "kind": "claude", "name": "Default" } } },
  "nodes": [ { "id": "plan", "slot": "dev" }, { "id": "review", "slot": "dev", "needs": ["plan"] } ] }"#;

struct Fixture {
    _directory: TempDir,
    path: PathBuf,
    editor: Entity<DefinitionEditor>,
}

/// An editor open on a hand-written `review.json`, after its first read of
/// the file.
fn open(cx: &mut TestAppContext) -> (Fixture, VisualTestContext) {
    let directory = tempdir().unwrap();
    let path = directory.path().join("review.json");

    fs::write(&path, HAND_WRITTEN).unwrap();

    let definition: Definition = serde_json::from_str(HAND_WRITTEN).unwrap();

    let (editor, window) = cx.update(|cx| {
        gpui_component::init(cx);

        cx.set_global(AgentSettings::default());

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(1000.), px(700.)),
            ))),
            ..WindowOptions::default()
        };

        let mut editor = None;

        let window = cx
            .open_window(options, |window, cx| {
                let view = cx
                    .new(|cx| DefinitionEditor::new("review".into(), path.clone(), definition, cx));

                editor = Some(view.clone());

                cx.new(|cx| Root::new(view, window, cx))
            })
            .unwrap();

        (editor.unwrap(), window)
    });

    let cx = VisualTestContext::from_window(window.into(), cx);

    cx.run_until_parked();

    let fixture = Fixture {
        _directory: directory,
        path,
        editor,
    };

    (fixture, cx)
}

fn move_plan(fixture: &Fixture, cx: &mut VisualTestContext) {
    fixture.editor.update(cx, |editor, cx| {
        assert!(editor.apply(
            Edit::MoveNodes(vec![("plan".into(), Position { x: 40., y: 50. })]),
            cx,
        ));
    });
}

fn check_file(fixture: &Fixture, cx: &mut VisualTestContext) {
    fixture
        .editor
        .update(cx, |editor, cx| editor.check_file(cx));

    cx.run_until_parked();
}

#[gpui::test]
async fn opening_a_hand_written_file_is_not_a_change(cx: &mut TestAppContext) {
    let (fixture, mut cx) = open(cx);

    fixture.editor.read_with(&cx, |editor, _| {
        assert!(!editor.is_dirty());
        assert!(editor.conflict.is_none());
        assert_eq!(editor.disk.as_deref(), Some(HAND_WRITTEN.as_bytes()));
    });

    check_file(&fixture, &mut cx);

    fixture
        .editor
        .read_with(&cx, |editor, _| assert!(editor.conflict.is_none()));
}

#[gpui::test]
async fn a_save_writes_the_canonical_format_and_is_not_an_external_edit(cx: &mut TestAppContext) {
    let (fixture, mut cx) = open(cx);

    move_plan(&fixture, &mut cx);

    let saved = fixture.editor.update(&mut cx, |editor, cx| editor.save(cx));

    cx.run_until_parked();

    assert!(saved.await);

    let expected = fixture.editor.read_with(&cx, |editor, _| {
        to_canonical_json(editor.editor.definition())
    });

    assert_eq!(fs::read_to_string(&fixture.path).unwrap(), expected);

    check_file(&fixture, &mut cx);

    fixture.editor.read_with(&cx, |editor, _| {
        assert!(!editor.is_dirty());
        assert!(editor.conflict.is_none());
        assert!(editor.editor.can_undo(), "saving keeps the history");
    });
}

#[gpui::test]
async fn an_external_edit_reloads_a_clean_canvas(cx: &mut TestAppContext) {
    let (fixture, mut cx) = open(cx);

    fs::write(&fixture.path, HAND_WRITTEN.replace("review", "check")).unwrap();

    check_file(&fixture, &mut cx);

    fixture.editor.read_with(&cx, |editor, _| {
        assert_eq!(editor.editor.definition().nodes[1].id, "check");
        assert!(editor.conflict.is_none());
        assert!(
            editor.reload_notice.is_some(),
            "a silent reload is announced"
        );
    });
}

#[gpui::test]
async fn an_external_edit_under_unsaved_edits_asks(cx: &mut TestAppContext) {
    let (fixture, mut cx) = open(cx);

    move_plan(&fixture, &mut cx);

    fs::write(&fixture.path, HAND_WRITTEN.replace("review", "check")).unwrap();

    check_file(&fixture, &mut cx);

    fixture.editor.read_with(&cx, |editor, _| {
        assert!(matches!(editor.conflict, Some(Conflict::Changed(Some(_)))));
        assert!(editor.is_dirty(), "the canvas keeps its edits");
    });

    fixture
        .editor
        .update(&mut cx, |editor, cx| editor.reload_from_conflict(cx));

    fixture.editor.read_with(&cx, |editor, _| {
        assert_eq!(editor.editor.definition().nodes[1].id, "check");
        assert!(!editor.is_dirty());
        assert!(editor.conflict.is_none());
    });
}

#[gpui::test]
async fn keeping_mine_and_saving_writes_over_the_file(cx: &mut TestAppContext) {
    let (fixture, mut cx) = open(cx);

    move_plan(&fixture, &mut cx);

    fs::write(&fixture.path, HAND_WRITTEN.replace("review", "check")).unwrap();

    check_file(&fixture, &mut cx);

    fixture
        .editor
        .update(&mut cx, |editor, cx| editor.keep_mine(cx));

    let saved = fixture.editor.update(&mut cx, |editor, cx| editor.save(cx));

    cx.run_until_parked();

    assert!(saved.await);

    let written: Definition =
        serde_json::from_str(&fs::read_to_string(&fixture.path).unwrap()).unwrap();

    assert_eq!(written.nodes[1].id, "review");
    assert_eq!(written.layout["plan"], Position { x: 40., y: 50. });
}

#[gpui::test]
async fn a_deleted_file_is_reported_and_recreated_on_save(cx: &mut TestAppContext) {
    let (fixture, mut cx) = open(cx);

    fs::remove_file(&fixture.path).unwrap();

    check_file(&fixture, &mut cx);

    fixture.editor.read_with(&cx, |editor, _| {
        assert!(matches!(editor.conflict, Some(Conflict::Deleted)));
    });

    let saved = fixture.editor.update(&mut cx, |editor, cx| editor.save(cx));

    cx.run_until_parked();

    assert!(saved.await);
    assert!(fixture.path.exists());

    fixture
        .editor
        .read_with(&cx, |editor, _| assert!(editor.conflict.is_none()));
}
