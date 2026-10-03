//! The orchestration definitions saved in the configuration directory.

#[cfg(test)]
#[path = "library_tests.rs"]
mod library_tests;

use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::orchestration::definition::Definition;
use crate::orchestration::graph::Graph;

const DEFINITIONS_DIRECTORY: [&str; 2] = ["agent-orchestrations", "definitions"];
const DEFINITION_EXTENSION: &str = "json";

/// One definition file, valid or not, named by its file stem.
#[derive(Clone, Debug)]
pub struct DefinitionEntry {
    pub name: String,
    pub path: PathBuf,
    pub graph: Result<Graph, Vec<String>>,
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

        let graph = read_definition(&path);

        definitions.push(DefinitionEntry { name, path, graph });
    }

    definitions.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(definitions)
}

fn read_definition(path: &Path) -> Result<Graph, Vec<String>> {
    let text = fs::read_to_string(path).map_err(|error| vec![error.to_string()])?;

    let definition: Definition =
        serde_json::from_str(&text).map_err(|error| vec![error.to_string()])?;

    Graph::new(definition).map_err(|errors| errors.iter().map(ToString::to_string).collect())
}
