//! Saved orchestration runs: one directory per run under
//! `agent-orchestrations/runs` in the data directory.
//!
//! `run.json` holds the [`RunRecord`] as a whole-state snapshot. The text
//! sent for each node and each node's output are separate files, because a
//! snapshot is rewritten on every state change and long outputs would make
//! each of those writes cost as much as all outputs together. A file is
//! written before the snapshot that refers to it, so a crash between the two
//! leaves an unreferenced file and never a recorded state without its text.

#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;

use std::path::{Path, PathBuf};
use std::{fs, io};

use nmt_platform::durable_file;
use thiserror::Error;

use crate::orchestration::run::{RunId, RunRecord, RunState};
use crate::snapshot_store::{self, SnapshotError, SnapshotFormat, SnapshotStore};

const FORMAT: SnapshotFormat = SnapshotFormat {
    file: "run.json",
    field: "run",
    version: 1,
};

const ORCHESTRATIONS_DIRECTORY: &str = "agent-orchestrations";
const RUNS_DIRECTORY: &str = "runs";
const PROMPTS_DIRECTORY: &str = "prompts";
const OUTPUTS_DIRECTORY: &str = "outputs";

#[derive(Debug, Error)]
pub enum RunStoreError {
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    #[error("run storage is unavailable: {0}")]
    Io(#[from] io::Error),
    #[error("the saved run does not match its directory")]
    IdentityChanged,
    #[error("a saved run cannot change its definition or input")]
    DefinitionChanged,
}

/// One run, owned by this NiumaTerm instance while the store is open.
pub struct RunStore {
    snapshots: SnapshotStore<RunRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSummary {
    pub id: RunId,
    pub definition_name: String,
    pub state: RunState,
    pub started_at: u64,
}

impl RunStore {
    pub fn create(data_directory: &Path, run: RunRecord) -> Result<Self, RunStoreError> {
        let parent = runs_directory(data_directory);

        fs::create_dir_all(&parent)?;

        let directory = parent.join(run.id().to_string());
        let snapshots = SnapshotStore::create(directory.clone(), &FORMAT, run)?;

        fs::create_dir(directory.join(PROMPTS_DIRECTORY))?;
        fs::create_dir(directory.join(OUTPUTS_DIRECTORY))?;

        Ok(Self { snapshots })
    }

    pub fn open(data_directory: &Path, id: RunId) -> Result<Self, RunStoreError> {
        let directory = runs_directory(data_directory).join(id.to_string());
        let snapshots = SnapshotStore::<RunRecord>::open(directory, &FORMAT)?;

        if snapshots.value().id() != id {
            return Err(RunStoreError::IdentityChanged);
        }

        Ok(Self { snapshots })
    }

    pub fn run(&self) -> &RunRecord {
        self.snapshots.value()
    }

    /// Apply `change` to a copy of the run and save the copy. The saved run
    /// is unchanged when saving fails, and an unchanged copy writes nothing.
    pub fn update<T>(
        &mut self,
        change: impl FnOnce(&mut RunRecord) -> T,
    ) -> Result<T, RunStoreError> {
        let mut next = self.snapshots.value().clone();

        let result = change(&mut next);

        self.snapshots.commit(next, |previous, next| {
            let same_run = previous.id() == next.id()
                && previous.definition() == next.definition()
                && previous.input() == next.input();

            if same_run {
                Ok(())
            } else {
                Err(RunStoreError::DefinitionChanged)
            }
        })?;

        Ok(result)
    }

    /// Save the text composed for `node`, before the node is recorded as
    /// sending. Sending a node again replaces it.
    pub fn save_prompt(&self, node: usize, text: &str) -> Result<(), RunStoreError> {
        Ok(durable_file::write(
            &self.node_file(PROMPTS_DIRECTORY, node),
            text.as_bytes(),
        )?)
    }

    /// Save `node`'s output, before the node is recorded as completed.
    pub fn save_output(&self, node: usize, text: &str) -> Result<(), RunStoreError> {
        Ok(durable_file::write(
            &self.node_file(OUTPUTS_DIRECTORY, node),
            text.as_bytes(),
        )?)
    }

    /// The text last sent for `node`, if any was sent.
    pub fn prompt(&self, node: usize) -> Result<Option<String>, RunStoreError> {
        read_optional(&self.node_file(PROMPTS_DIRECTORY, node))
    }

    /// `node`'s output when the run records it as completed. An output file
    /// for a node that is not completed belongs to a turn whose completion
    /// was never saved, and is not shown.
    pub fn output(&self, node: usize) -> Result<Option<String>, RunStoreError> {
        if !self.run().nodes()[node].state.is_completed() {
            return Ok(None);
        }

        read_optional(&self.node_file(OUTPUTS_DIRECTORY, node))
    }

    /// Node ids are restricted to characters that are valid in file names
    /// on every platform, so they name the files directly.
    fn node_file(&self, kind: &str, node: usize) -> PathBuf {
        let id = &self.run().definition().nodes[node].id;

        self.snapshots
            .directory()
            .join(kind)
            .join(format!("{id}.md"))
    }
}

/// Summaries of the runs started in `workspace`, newest first, read without
/// taking any run's lock. A damaged run is skipped so it cannot hide the
/// others.
pub fn recent_runs(
    data_directory: &Path,
    workspace: Option<&str>,
) -> Result<Vec<RunSummary>, RunStoreError> {
    let directory = runs_directory(data_directory);

    if !directory.exists() {
        return Ok(Vec::new());
    }

    let mut summaries = Vec::new();

    for entry in fs::read_dir(directory)? {
        let entry = entry?;

        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<RunId>().ok())
        else {
            continue;
        };

        match snapshot_store::read::<RunRecord>(&entry.path().join(FORMAT.file), &FORMAT) {
            Ok((_, run)) if run.id() != id => {
                tracing::warn!(%id, "skipping an orchestration run saved under another id");
            }
            Ok((_, run)) if run.workspace() == workspace => summaries.push(RunSummary {
                id,
                definition_name: run.definition_name().to_owned(),
                state: run.state(),
                started_at: run.started_at(),
            }),
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%id, %error, "could not read an orchestration run");
            }
        }
    }

    summaries.sort_by(|left, right| {
        right
            .started_at
            .cmp(&left.started_at)
            .then_with(|| left.id.cmp(&right.id))
    });

    Ok(summaries)
}

fn runs_directory(data_directory: &Path) -> PathBuf {
    data_directory
        .join(ORCHESTRATIONS_DIRECTORY)
        .join(RUNS_DIRECTORY)
}

fn read_optional(path: &Path) -> Result<Option<String>, RunStoreError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
