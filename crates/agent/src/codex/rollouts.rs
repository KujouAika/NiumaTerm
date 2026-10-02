//! Codex conversations read straight from the rollout files the CLI records,
//! for a history list that has no Codex app-server of its own to ask.
//!
//! Every Codex thread is persisted as one JSON Lines file under
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<timestamp>-<id>.jsonl`. Its first
//! record is the `session_meta` line naming the thread id, working directory,
//! model provider, launch source, and git state; the user's prompts follow as
//! `event_msg` records of type `user_message`. A name given to a thread is not
//! in its rollout but appended to `$CODEX_HOME/session_index.jsonl`, where the
//! newest line for an id wins.

#[cfg(test)]
#[path = "rollouts_tests.rs"]
mod rollouts_tests;

use std::cmp::Reverse;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{env, fs};

use nmt_platform::environment::home_dir;
use nmt_platform::filesystem::path_identity;
use serde_json::Value;

use crate::chat::{SessionScope, SessionSummary};
use crate::profile::CODEX_PROVIDER_PREFIX;
use crate::session::naming::provisional_title;

/// Head window scanned for the session metadata and the first prompt. The
/// metadata line embeds the base instructions and can run to tens of
/// kilobytes, and a thread opens with environment context records before the
/// prompt; a rollout whose prompt lies past this falls back to its id title.
const HEAD_SCAN_BYTES: u64 = 256 * 1024;

/// Rows one listing returns at most. Every candidate's head has to be read to
/// learn its directory, so the walk stops once this many matched rather than
/// parsing years of rollouts for a list that shows the newest few.
const LISTING_LIMIT: usize = 200;

const PROVISIONAL_TITLE_WORDS: usize = 6;

/// Which model providers' threads a listing keeps. Codex records the provider
/// a thread ran against, and a resume only finds the thread again under that
/// provider, so a profile lists the threads it can continue.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProviderFilter {
    /// Only threads recorded against this provider id.
    Only(String),
    /// Threads of every provider except the ones this application generates
    /// for custom-endpoint profiles, which belong to those profiles.
    ExceptGenerated,
}

impl ProviderFilter {
    fn admits(&self, provider: Option<&str>) -> bool {
        match self {
            Self::Only(id) => provider == Some(id.as_str()),
            Self::ExceptGenerated => {
                !provider.is_some_and(|provider| provider.starts_with(CODEX_PROVIDER_PREFIX))
            }
        }
    }
}

/// The Codex home a launch environment selects: `CODEX_HOME` when the launch
/// or this process sets it, otherwise `~/.codex`.
pub(crate) fn codex_home(launch_env: &[(String, String)]) -> Option<PathBuf> {
    launch_env
        .iter()
        .rev()
        .find(|(name, _)| name == "CODEX_HOME")
        .map(|(_, value)| value.clone())
        .or_else(|| env::var("CODEX_HOME").ok())
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".codex")))
}

/// Threads recorded under `codex_home` that `scope` covers from `cwd` and
/// `providers` admits, newest first. Meant for a background thread: it walks
/// the rollout tree and reads a bounded head of each candidate.
pub fn list_sessions(
    codex_home: &Path,
    scope: SessionScope,
    cwd: Option<&str>,
    providers: &ProviderFilter,
) -> Vec<SessionSummary> {
    let names = thread_names(&codex_home.join("session_index.jsonl"));

    let target = match scope {
        SessionScope::CurrentDirectory => cwd.map(|cwd| path_identity(Path::new(cwd))),
        SessionScope::AllDirectories => None,
    };

    let mut candidates = Vec::new();

    collect_rollouts(&codex_home.join("sessions"), &mut candidates);

    candidates.sort_by_key(|(_, modified)| Reverse(*modified));

    let mut sessions = Vec::new();

    for (path, last_active) in candidates {
        if sessions.len() >= LISTING_LIMIT {
            break;
        }

        let Some(head) = read_head(&path) else {
            continue;
        };

        if head.subagent || !providers.admits(head.provider.as_deref()) {
            continue;
        }

        // A thread that never received a prompt has nothing to continue.
        let Some(prompt) = head.first_prompt else {
            continue;
        };

        if let Some(target) = &target {
            let here = head
                .cwd
                .as_deref()
                .is_some_and(|cwd| path_identity(Path::new(cwd)) == *target);

            if !here {
                continue;
            }
        }

        let title = names
            .get(&head.id)
            .cloned()
            .or_else(|| provisional_title(&prompt, Some(PROVISIONAL_TITLE_WORDS)))
            .unwrap_or_else(|| head.id.chars().take(8).collect());

        sessions.push(SessionSummary {
            id: head.id,
            title,
            branch: head.branch,
            cwd: head.cwd,
            last_active,
            snippet: None,
            origin: None,
        });
    }

    sessions
}

/// Every `rollout-*.jsonl` below `dir`, with its modification time. The tree
/// is date-partitioned a few levels deep, so this recurses rather than
/// assuming the depth.
fn collect_rollouts(dir: &Path, out: &mut Vec<(PathBuf, SystemTime)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();

        let Ok(metadata) = entry.metadata() else {
            continue;
        };

        if metadata.is_dir() {
            collect_rollouts(&path, out);

            continue;
        }

        let is_rollout = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"));

        if is_rollout && let Ok(modified) = metadata.modified() {
            out.push((path, modified));
        }
    }
}

/// What the head of a rollout says about its thread.
#[derive(Debug, Default, PartialEq)]
struct RolloutHead {
    id: String,
    cwd: Option<String>,
    branch: Option<String>,
    provider: Option<String>,
    subagent: bool,
    first_prompt: Option<String>,
}

fn read_head(path: &Path) -> Option<RolloutHead> {
    let file = fs::File::open(path).ok()?;

    parse_head(BufReader::new(file.take(HEAD_SCAN_BYTES)))
}

/// Read the session metadata and the first prompt from the head of a rollout.
/// A rollout without a metadata record is not a thread this can name, so it
/// yields nothing; a head cut off by the scan window keeps what it read.
fn parse_head(reader: impl BufRead) -> Option<RolloutHead> {
    let mut head: Option<RolloutHead> = None;

    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };

        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let payload = &record["payload"];

        match (record["type"].as_str(), payload["type"].as_str()) {
            (Some("session_meta"), _) if head.is_none() => {
                let id = payload["id"].as_str()?.to_string();

                head = Some(RolloutHead {
                    id,
                    cwd: non_empty(&payload["cwd"]),
                    branch: non_empty(&payload["git"]["branch"]),
                    provider: non_empty(&payload["model_provider"]),
                    // A subagent's thread belongs to the parent that spawned
                    // it rather than to a list of conversations to reopen.
                    subagent: payload["source"].get("subagent").is_some(),
                    first_prompt: None,
                });
            }
            (Some("event_msg"), Some("user_message")) => {
                let Some(head) = head.as_mut() else {
                    continue;
                };

                if let Some(message) = non_empty(&payload["message"]) {
                    head.first_prompt = Some(message);

                    break;
                }
            }
            _ => {}
        }
    }

    head
}

/// The newest name recorded for each thread id. Names are appended, so a
/// later line for an id supersedes the earlier ones, and an empty name clears
/// it back to the prompt the thread opened with.
fn thread_names(path: &Path) -> HashMap<String, String> {
    let Ok(file) = fs::File::open(path) else {
        return HashMap::new();
    };

    parse_thread_names(BufReader::new(file))
}

fn parse_thread_names(reader: impl BufRead) -> HashMap<String, String> {
    let mut names = HashMap::new();

    for line in reader.lines().map_while(Result::ok) {
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let Some(id) = record["id"].as_str() else {
            continue;
        };

        match non_empty(&record["thread_name"]) {
            Some(name) => names.insert(id.to_string(), name),
            None => names.remove(id),
        };
    }

    names
}

fn non_empty(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
