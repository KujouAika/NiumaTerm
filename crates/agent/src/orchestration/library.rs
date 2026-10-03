//! The orchestration definitions saved in the configuration directory.

#[cfg(test)]
#[path = "library_tests.rs"]
mod library_tests;

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::{fs, io};

use thiserror::Error;

use crate::orchestration::canonical::to_canonical_json;
use crate::orchestration::definition::{DEFINITION_VERSION, Definition};
use crate::orchestration::graph::Graph;

const DEFINITIONS_DIRECTORY: [&str; 2] = ["agent-orchestrations", "definitions"];
const DEFINITION_EXTENSION: &str = "json";

/// One definition file, valid or not, named by its file stem.
#[derive(Clone, Debug)]
pub struct DefinitionEntry {
    pub name: String,
    pub path: PathBuf,

    /// The decoded definition, `None` when the file does not decode. A
    /// definition that decodes but does not validate is still shown and
    /// edited on the canvas.
    pub definition: Option<Definition>,

    /// The validated graph, or every error that kept the file from decoding
    /// or validating.
    pub graph: Result<Graph, Vec<String>>,
}

#[derive(Debug, Error)]
pub enum CreateError {
    #[error("a definition name uses only letters, digits, `_` and `-`")]
    InvalidName,
    #[error("a definition named `{0}` already exists")]
    NameInUse(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub fn definitions_directory(data_directory: &Path) -> PathBuf {
    DEFINITIONS_DIRECTORY
        .iter()
        .fold(data_directory.to_owned(), |path, part| path.join(part))
}

/// Every definition file, by name. A file that cannot be read, decoded or
/// validated is listed with its errors, so a mistake in one file is shown
/// next to it instead of hiding it. A missing directory lists nothing.
pub fn load_definitions(data_directory: &Path) -> io::Result<Vec<DefinitionEntry>> {
    let directory = definitions_directory(data_directory);

    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    let mut definitions = Vec::new();

    for entry in entries {
        let path = entry?.path();

        let is_definition = path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == DEFINITION_EXTENSION);

        let Some(name) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|_| is_definition)
            .map(str::to_owned)
        else {
            continue;
        };

        let (definition, graph) = read_definition(&path);

        definitions.push(DefinitionEntry {
            name,
            path,
            definition,
            graph,
        });
    }

    definitions.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(definitions)
}

fn read_definition(path: &Path) -> (Option<Definition>, Result<Graph, Vec<String>>) {
    let decoded = fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            serde_json::from_str::<Definition>(&text).map_err(|error| error.to_string())
        });

    match decoded {
        Ok(definition) => {
            let graph = Graph::new(definition.clone())
                .map_err(|errors| errors.iter().map(ToString::to_string).collect());

            (Some(definition), graph)
        }
        Err(error) => (None, Err(vec![error])),
    }
}

/// Write a new definition file named `name` holding only the version and
/// no slots or nodes, and return its path. A name already in use is
/// refused, also when another program creates the file at the same moment,
/// because the file is opened only if it does not exist yet.
pub fn create_definition(data_directory: &Path, name: &str) -> Result<PathBuf, CreateError> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'));

    if !valid {
        return Err(CreateError::InvalidName);
    }

    let directory = definitions_directory(data_directory);

    fs::create_dir_all(&directory)?;

    let path = directory.join(format!("{name}.{DEFINITION_EXTENSION}"));

    let empty = Definition {
        version: DEFINITION_VERSION,
        max_parallel: None,
        slots: Vec::new(),
        nodes: Vec::new(),
        layout: BTreeMap::new(),
    };

    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(CreateError::NameInUse(name.to_owned()));
        }
        Err(error) => return Err(error.into()),
    };

    file.write_all(to_canonical_json(&empty).as_bytes())?;

    Ok(path)
}
