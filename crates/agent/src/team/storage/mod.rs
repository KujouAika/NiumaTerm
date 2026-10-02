pub mod history;

mod validation;

#[cfg(test)]
mod tests;

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{io, thread};

use nmt_platform::durable_file;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::team::model::RoomId;
use crate::team::room::Room;

const VERSION: u32 = 3;

/// How long opening a room waits out a lock that may only be held by a child
/// process between fork and exec.
const LOCK_RETRY_WINDOW: Duration = Duration::from_millis(250);

const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(5);

/// The directory under the data directory that holds one directory per room.
const ROOMS_DIRECTORY: &str = "agent-teams";

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("room storage is unavailable: {0}")]
    Io(#[from] io::Error),
    #[error("room snapshot cannot be decoded: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported Team data version {0}")]
    UnsupportedVersion(u64),
    #[error("room snapshot failed validation: {0}")]
    Invalid(&'static str),
    #[error("legacy Team room format is not supported; saved files were left unchanged")]
    LegacyFormat,
    #[error("reopen this room to reconcile a failed storage operation")]
    ReopenRequired,
}

pub struct RoomStore {
    directory: PathBuf,
    _lock: File,
    room: Room,
    revision: u64,
    failed: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    revision: u64,
    room: Room,
}

impl RoomStore {
    pub fn create(data_directory: &Path, room: Room) -> Result<Self, StorageError> {
        validation::validate(&room)?;

        let parent = data_directory.join(ROOMS_DIRECTORY);

        fs::create_dir_all(&parent)?;

        let directory = parent.join(room.id().to_string());

        fs::create_dir(&directory)?;

        let lock = lock_room(&directory)?;

        write_snapshot(&directory, &room, 0)?;

        Ok(Self {
            directory,
            _lock: lock,
            room,
            revision: 0,
            failed: false,
        })
    }

    /// The rooms saved under `data_directory`, in id order.
    pub(crate) fn saved_rooms(data_directory: &Path) -> Result<Vec<RoomId>, StorageError> {
        let directory = data_directory.join(ROOMS_DIRECTORY);

        if !directory.exists() {
            return Ok(Vec::new());
        }

        let mut rooms = Vec::new();

        for entry in fs::read_dir(directory)? {
            let entry = entry?;

            if entry.file_type()?.is_dir()
                && let Some(id) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse().ok())
            {
                rooms.push(id);
            }
        }

        rooms.sort();

        Ok(rooms)
    }

    pub fn open(data_directory: &Path, id: RoomId) -> Result<Self, StorageError> {
        let directory = data_directory.join(ROOMS_DIRECTORY).join(id.to_string());
        let lock = lock_room(&directory)?;

        let bytes = match fs::read(directory.join("room.json")) {
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && (directory.join("checkpoint.json").exists()
                        || directory.join("journal.jsonl").exists()) =>
            {
                return Err(StorageError::LegacyFormat);
            }
            result => result?,
        };

        let raw: Value = serde_json::from_slice(&bytes)?;

        let version = raw
            .get("version")
            .and_then(Value::as_u64)
            .ok_or(StorageError::Invalid("missing room version"))?;

        if version != u64::from(VERSION) {
            return Err(StorageError::UnsupportedVersion(version));
        }

        let snapshot: Snapshot = serde_json::from_value(raw)?;

        if snapshot.room.id() != id {
            return Err(StorageError::Invalid("room identity changed"));
        }

        validation::validate(&snapshot.room)?;

        Ok(Self {
            directory,
            _lock: lock,
            room: snapshot.room,
            revision: snapshot.revision,
            failed: false,
        })
    }

    pub fn room(&self) -> &Room {
        &self.room
    }

    pub fn data_directory(&self) -> &Path {
        self.directory
            .parent()
            .and_then(Path::parent)
            .expect("room directories are nested beneath the data directory")
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Publish memory only after the complete snapshot reaches durable storage.
    /// A failed replacement can have reached disk, so reopen before another write.
    pub fn commit(&mut self, next: Room) -> Result<(), StorageError> {
        if self.failed {
            return Err(StorageError::ReopenRequired);
        }

        if next == self.room {
            return Ok(());
        }

        validation::validate(&next)?;
        validation::validate_update(&self.room, &next)?;

        let revision = self
            .revision
            .checked_add(1)
            .ok_or(StorageError::Invalid("room revision exhausted"))?;

        if let Err(error) = write_snapshot(&self.directory, &next, revision) {
            self.failed = true;

            return Err(error);
        }

        self.room = next;
        self.revision = revision;

        Ok(())
    }
}

fn write_snapshot(directory: &Path, room: &Room, revision: u64) -> Result<(), StorageError> {
    let snapshot = Snapshot {
        version: VERSION,
        revision,
        room: room.clone(),
    };

    durable_file::write(
        &directory.join("room.json"),
        &serde_json::to_vec(&snapshot)?,
    )?;

    Ok(())
}

fn lock_room(directory: &Path) -> Result<File, StorageError> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("owner.lock"))?;

    // The lock belongs to the open file rather than to this descriptor, and a
    // child forked by another thread holds a copy of every open file until it
    // execs, the close-on-exec flag notwithstanding. A PTY child runs its
    // setup between the two, so a room just released here can read as locked
    // for a few milliseconds. Retrying briefly waits that out, while a room
    // another instance really holds still fails within a moment.
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
