use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::json;

use crate::chat::SessionScope;
use crate::codex::rollouts::{
    ProviderFilter, RolloutHead, codex_home, list_sessions, parse_head, parse_thread_names,
};

fn meta(id: &str, cwd: &str, provider: &str, source: serde_json::Value) -> String {
    json!({
        "timestamp": "2026-09-01T10:00:00.000Z",
        "type": "session_meta",
        "payload": {
            "id": id,
            "timestamp": "2026-09-01T10:00:00.000Z",
            "cwd": cwd,
            "originator": "codex_cli_rs",
            "cli_version": "0.153.4",
            "instructions": "x".repeat(4096),
            "source": source,
            "model_provider": provider,
            "git": { "commit_hash": "abc", "branch": "main" }
        }
    })
    .to_string()
}

fn environment_context() -> String {
    json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "<environment_context>cwd</environment_context>" }]
        }
    })
    .to_string()
}

fn user_message(text: &str) -> String {
    json!({
        "type": "event_msg",
        "payload": { "type": "user_message", "message": text, "images": [] }
    })
    .to_string()
}

fn write_rollout(home: &Path, day: &str, id: &str, lines: &[String], modified: SystemTime) {
    let dir = home.join("sessions").join(day);

    fs::create_dir_all(&dir).unwrap();

    let path = dir.join(format!("rollout-2026-09-01T10-00-00-{id}.jsonl"));

    fs::write(&path, lines.join("\n") + "\n").unwrap();

    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

#[test]
fn head_takes_the_typed_prompt_rather_than_injected_context() {
    let rollout = [
        meta("t-1", "/work/app", "openai", json!("cli")),
        environment_context(),
        user_message("fix the flaky login test"),
        user_message("and the second one"),
    ]
    .join("\n");

    let head = parse_head(Cursor::new(rollout)).unwrap();

    assert_eq!(
        head,
        RolloutHead {
            id: "t-1".into(),
            cwd: Some("/work/app".into()),
            branch: Some("main".into()),
            provider: Some("openai".into()),
            subagent: false,
            first_prompt: Some("fix the flaky login test".into()),
        }
    );
}

#[test]
fn head_without_metadata_names_no_thread() {
    assert_eq!(parse_head(Cursor::new(user_message("hello"))), None);
}

#[test]
fn newest_thread_name_wins_and_an_empty_one_clears_it() {
    let index = [
        json!({ "id": "a", "thread_name": "First name", "updated_at": "2026-09-01T00:00:00Z" }),
        json!({ "id": "b", "thread_name": "Kept", "updated_at": "2026-09-01T00:00:00Z" }),
        json!({ "id": "a", "thread_name": "Renamed", "updated_at": "2026-09-02T00:00:00Z" }),
        json!({ "id": "b", "thread_name": "", "updated_at": "2026-09-03T00:00:00Z" }),
    ]
    .map(|line| line.to_string())
    .join("\n");

    let names = parse_thread_names(Cursor::new(index));

    assert_eq!(names.get("a").map(String::as_str), Some("Renamed"));
    assert_eq!(names.get("b"), None);
}

#[test]
fn listing_keeps_resumable_threads_of_the_directory_newest_first() {
    let home = tempfile::tempdir().unwrap();
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);

    write_rollout(
        home.path(),
        "2026/09/01",
        "older",
        &[
            meta("older", "/work/app", "openai", json!("cli")),
            user_message("older prompt about the parser"),
        ],
        base,
    );

    write_rollout(
        home.path(),
        "2026/09/02",
        "newer",
        &[
            meta("newer", "/work/app", "openai", json!("vscode")),
            user_message("newer prompt"),
        ],
        base + Duration::from_secs(60),
    );

    write_rollout(
        home.path(),
        "2026/09/02",
        "elsewhere",
        &[
            meta("elsewhere", "/work/other", "openai", json!("cli")),
            user_message("other project"),
        ],
        base + Duration::from_secs(120),
    );

    write_rollout(
        home.path(),
        "2026/09/02",
        "blank",
        &[meta("blank", "/work/app", "openai", json!("cli"))],
        base + Duration::from_secs(180),
    );

    write_rollout(
        home.path(),
        "2026/09/02",
        "child",
        &[
            meta(
                "child",
                "/work/app",
                "openai",
                json!({ "subagent": { "thread_spawn": {} } }),
            ),
            user_message("delegated work"),
        ],
        base + Duration::from_secs(240),
    );

    write_rollout(
        home.path(),
        "2026/09/02",
        "custom",
        &[
            meta(
                "custom",
                "/work/app",
                "niumaterm-0123456789abcdef",
                json!("vscode"),
            ),
            user_message("through the proxy"),
        ],
        base + Duration::from_secs(300),
    );

    fs::write(
        home.path().join("session_index.jsonl"),
        json!({ "id": "older", "thread_name": "Parser cleanup" }).to_string() + "\n",
    )
    .unwrap();

    let listed = list_sessions(
        home.path(),
        SessionScope::CurrentDirectory,
        Some("/work/app"),
        &ProviderFilter::ExceptGenerated,
    );

    let rows: Vec<_> = listed
        .iter()
        .map(|row| (row.id.as_str(), row.title.as_str()))
        .collect();

    assert_eq!(
        rows,
        [("newer", "newer prompt"), ("older", "Parser cleanup")]
    );
    assert_eq!(listed[0].branch.as_deref(), Some("main"));
    assert_eq!(listed[0].cwd.as_deref(), Some("/work/app"));
    assert_eq!(listed[0].last_active, base + Duration::from_secs(60));

    let everywhere = list_sessions(
        home.path(),
        SessionScope::AllDirectories,
        Some("/work/app"),
        &ProviderFilter::ExceptGenerated,
    );

    assert_eq!(
        everywhere
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["elsewhere", "newer", "older"]
    );

    let custom = list_sessions(
        home.path(),
        SessionScope::CurrentDirectory,
        Some("/work/app"),
        &ProviderFilter::Only("niumaterm-0123456789abcdef".into()),
    );

    assert_eq!(
        custom.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["custom"]
    );
}

#[test]
fn launch_environment_selects_the_codex_home() {
    let home = codex_home(&[("CODEX_HOME".to_string(), "/profiles/codex".to_string())]);

    assert_eq!(home.as_deref(), Some(Path::new("/profiles/codex")));
}
