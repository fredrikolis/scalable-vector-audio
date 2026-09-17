// Concern: which directory a subcommand reads its composition from | Non-concern: loading it (sva-core), where a reading is written (destination.rs) | IO: (--in, or none) -> a directory or a refusal

use std::io::ErrorKind;
use std::path::PathBuf;

use sva_core::{CliError, cwd};

/// `--in` where the caller named a directory, the process's own where it did not. Either way
/// the path is read here, so a caller is told which directory answered nothing rather than
/// meeting the graph's own refusal for a tree that was never there.
pub fn composition(named: Option<&str>) -> Result<PathBuf, CliError> {
    let dir = match named {
        Some(path) => PathBuf::from(path),
        None => cwd()?,
    };
    match std::fs::read_dir(&dir) {
        Ok(_) => Ok(dir),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(CliError::NotFound(format!(
            "no composition at `{}`",
            dir.display()
        ))),
        Err(e) if e.kind() == ErrorKind::NotADirectory => Err(CliError::Usage(format!(
            "`{}` is a file; a composition is the directory its nodes sit in",
            dir.display()
        ))),
        Err(e) => Err(CliError::Io(format!(
            "could not read the composition at `{}`: {e}",
            dir.display()
        ))),
    }
}
