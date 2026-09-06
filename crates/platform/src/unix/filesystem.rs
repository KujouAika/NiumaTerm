use std::path::Path;
use std::{fs, io};

/// Move `source` over `destination`, replacing it if it exists.
///
/// POSIX `rename` already replaces an existing destination atomically within
/// one filesystem, so no separate replace call is needed here.
pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}
