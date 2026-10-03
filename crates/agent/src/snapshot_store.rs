//! A directory that saves one value as a whole-state snapshot.
//!
//! Every commit rewrites the complete value, so opening reads one file and
//! never replays a log, while a write costs time in proportion to the whole
//! value, not to the change. A store suits state that stays small or
//! changes rarely; large or append-heavy data belongs in separate files that
//! the snapshot only references.

#[cfg(test)]
#[path = "snapshot_store_tests.rs"]
mod snapshot_store_tests;

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{io, thread};

use nmt_platform::durable_file;
use serde::de::DeserializeOwned;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;

/// How long opening a store waits out a lock that may only be held by a child
/// process between fork and exec.
const LOCK_RETRY_WINDOW: Duration = Duration::from_millis(250);

const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(5);

const LOCK_FILE: &str = "owner.lock";

/// The file layout of one kind of snapshot.
pub(crate) struct SnapshotFormat {
    /// The snapshot's file name inside the store directory.
    pub file: &'static str,

    /// The top-level key that holds the value, beside `version` and
    /// `revision`. Each format names its own so files written before this
    /// store existed keep decoding.
    pub field: &'static str,

    /// Opening rejects any other version without touching the saved bytes.
    pub version: u32,
}

#[derive(Debug, Error)]
pub(crate) enum SnapshotError {
    #[error("snapshot storage is unavailable: {0}")]
    Io(#[from] io::Error),
    #[error("snapshot cannot be decoded: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported snapshot version {0}")]
    UnsupportedVersion(u64),
    #[error("snapshot failed validation: {0}")]
    Invalid(&'static str),
    #[error("reopen to reconcile a failed storage operation")]
    ReopenRequired,
}

/// One value owned by this process and saved under `directory`.
///
/// The memory copy changes only after its snapshot reaches durable storage,
/// so `value` always matches a state some later `open` can return.
pub(crate) struct SnapshotStore<T> {
    directory: PathBuf,
    format: &'static SnapshotFormat,
    _lock: File,
    value: T,
    revision: u64,

    /// A failed replacement can still have reached disk, so the memory copy
    /// may be older than the file and every later write is refused.
    failed: bool,
}

impl<T: Serialize + DeserializeOwned + PartialEq> SnapshotStore<T> {
    /// Create `directory` and save `value` as revision 0. An existing
    /// directory is an error, so two values can never share one store.
    pub(crate) fn create(
        directory: PathBuf,
        format: &'static SnapshotFormat,
        value: T,
    ) -> Result<Self, SnapshotError> {
        fs::create_dir(&directory)?;

        let lock = lock(&directory)?;

        write(&directory, format, &value, 0)?;

        Ok(Self {
            directory,
            format,
            _lock: lock,
            value,
            revision: 0,
            failed: false,
        })
    }

    /// Take ownership of `directory` and read its snapshot. A missing
    /// snapshot file returns the `NotFound` I/O error, so a caller can tell
    /// an older layout apart from damage.
    pub(crate) fn open(
        directory: PathBuf,
        format: &'static SnapshotFormat,
    ) -> Result<Self, SnapshotError> {
        let lock = lock(&directory)?;
        let (revision, value) = read(&directory.join(format.file), format)?;

        Ok(Self {
            directory,
            format,
            _lock: lock,
            value,
            revision,
            failed: false,
        })
    }

    pub(crate) fn value(&self) -> &T {
        &self.value
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// Save `next` and make it the memory copy. `check` receives the current
    /// and the next value and can refuse the change before anything is
    /// written. An unchanged value writes nothing and keeps its revision.
    pub(crate) fn commit<E: From<SnapshotError>>(
        &mut self,
        next: T,
        check: impl FnOnce(&T, &T) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.failed {
            return Err(SnapshotError::ReopenRequired.into());
        }

        if next == self.value {
            return Ok(());
        }

        check(&self.value, &next)?;

        let revision = self
            .revision
            .checked_add(1)
            .ok_or(SnapshotError::Invalid("snapshot revision exhausted"))?;

        if let Err(error) = write(&self.directory, self.format, &next, revision) {
            self.failed = true;

            return Err(error.into());
        }

        self.value = next;
        self.revision = revision;

        Ok(())
    }

    /// Redirect later writes, so a test can make them fail.
    #[cfg(test)]
    pub(crate) fn directory_mut(&mut self) -> &mut PathBuf {
        &mut self.directory
    }
}

/// Decode the snapshot at `path` without taking the store's lock, for
/// listings that must not block on, or be blocked by, a live owner.
pub(crate) fn read<T: DeserializeOwned>(
    path: &Path,
    format: &SnapshotFormat,
) -> Result<(u64, T), SnapshotError> {
    let raw: Value = serde_json::from_slice(&fs::read(path)?)?;

    let version = raw
        .get("version")
        .and_then(Value::as_u64)
        .ok_or(SnapshotError::Invalid("missing snapshot version"))?;

    if version != u64::from(format.version) {
        return Err(SnapshotError::UnsupportedVersion(version));
    }

    let Value::Object(mut fields) = raw else {
        return Err(SnapshotError::Invalid("snapshot is not an object"));
    };

    fields.remove("version");

    let revision = fields
        .remove("revision")
        .ok_or(SnapshotError::Invalid("missing snapshot revision"))?;

    let value = fields
        .remove(format.field)
        .ok_or(SnapshotError::Invalid("missing snapshot value"))?;

    // A field this build does not know was written by a newer or foreign
    // writer; saving over it would drop that data.
    if !fields.is_empty() {
        return Err(SnapshotError::Invalid("unknown snapshot field"));
    }

    Ok((
        serde_json::from_value(revision)?,
        serde_json::from_value(value)?,
    ))
}

/// The saved form of a snapshot. Serialized by hand because the value's key
/// is chosen per format at run time.
struct SnapshotDocument<'a, T> {
    format: &'a SnapshotFormat,
    revision: u64,
    value: &'a T,
}

impl<T: Serialize> Serialize for SnapshotDocument<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;

        map.serialize_entry("version", &self.format.version)?;
        map.serialize_entry("revision", &self.revision)?;
        map.serialize_entry(self.format.field, self.value)?;

        map.end()
    }
}

fn write<T: Serialize>(
    directory: &Path,
    format: &SnapshotFormat,
    value: &T,
    revision: u64,
) -> Result<(), SnapshotError> {
    let document = SnapshotDocument {
        format,
        revision,
        value,
    };

    durable_file::write(
        &directory.join(format.file),
        &serde_json::to_vec(&document)?,
    )?;

    Ok(())
}

fn lock(directory: &Path) -> Result<File, SnapshotError> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(LOCK_FILE))?;

    // The lock belongs to the open file, not to this descriptor, and a
    // child forked by another thread holds a copy of every open file until it
    // execs, the close-on-exec flag notwithstanding. A PTY child runs its
    // setup between the two, so a store just released here can read as
    // locked for a few milliseconds. Retrying briefly waits that out, while a
    // store another instance really holds still fails within a moment.
    let deadline = Instant::now() + LOCK_RETRY_WINDOW;

    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(error) => return Err(io::Error::other(error).into()),
        }
    }
}
