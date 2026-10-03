pub mod history;

mod validation;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::{fs, io};

use thiserror::Error;

use crate::snapshot_store::{SnapshotError, SnapshotFormat, SnapshotStore};
use crate::team::model::RoomId;
use crate::team::room::Room;

const FORMAT: SnapshotFormat = SnapshotFormat {
    file: "room.json",
    field: "room",
    version: 3,
};

/// Files of the checkpoint-and-journal layout that `room.json` replaced.
const LEGACY_FILES: [&str; 2] = ["checkpoint.json", "journal.jsonl"];

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

impl From<SnapshotError> for StorageError {
    fn from(error: SnapshotError) -> Self {
        match error {
            SnapshotError::Io(error) => Self::Io(error),
            SnapshotError::Json(error) => Self::Json(error),
            SnapshotError::UnsupportedVersion(version) => Self::UnsupportedVersion(version),
            SnapshotError::Invalid(reason) => Self::Invalid(reason),
            SnapshotError::ReopenRequired => Self::ReopenRequired,
        }
    }
}

pub struct RoomStore {
    snapshots: SnapshotStore<Room>,
}

impl RoomStore {
    pub fn create(data_directory: &Path, room: Room) -> Result<Self, StorageError> {
        validation::validate(&room)?;

        let parent = data_directory.join(ROOMS_DIRECTORY);

        fs::create_dir_all(&parent)?;

        let directory = parent.join(room.id().to_string());

        Ok(Self {
            snapshots: SnapshotStore::create(directory, &FORMAT, room)?,
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

        let snapshots = match SnapshotStore::<Room>::open(directory.clone(), &FORMAT) {
            Err(SnapshotError::Io(error))
                if error.kind() == io::ErrorKind::NotFound
                    && LEGACY_FILES
                        .iter()
                        .any(|file| directory.join(file).exists()) =>
            {
                return Err(StorageError::LegacyFormat);
            }
            result => result?,
        };

        if snapshots.value().id() != id {
            return Err(StorageError::Invalid("room identity changed"));
        }

        validation::validate(snapshots.value())?;

        Ok(Self { snapshots })
    }

    pub fn room(&self) -> &Room {
        self.snapshots.value()
    }

    pub fn data_directory(&self) -> &Path {
        self.snapshots
            .directory()
            .parent()
            .and_then(Path::parent)
            .expect("room directories are nested beneath the data directory")
    }

    pub fn revision(&self) -> u64 {
        self.snapshots.revision()
    }

    /// Publish memory only after the complete snapshot reaches durable storage.
    /// A failed replacement can have reached disk, so reopen before another write.
    pub fn commit(&mut self, next: Room) -> Result<(), StorageError> {
        self.snapshots.commit(next, |previous, next| {
            validation::validate(next)?;

            validation::validate_update(previous, next)
        })
    }
}
