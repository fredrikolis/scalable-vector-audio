// Concern: named bytes under one filesystem path, each write whole or absent | Non-concern: what the bytes mean, the budget (sva-engine) | IO: (name[, bytes]) -> bytes, names, or why not

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sva_core::Backend;

/// One implementation for a disk directory and `/dev/shm` alike: both are paths.
pub struct Directory {
    pub path: PathBuf,
}

static WRITES: AtomicU64 = AtomicU64::new(0);

fn failed(what: &str, path: &Path, e: std::io::Error) -> String {
    format!("could not {what} `{}`: {e}", path.display())
}

impl Backend for Directory {
    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let path = self.path.join(name);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(failed("read", &path, e)),
        }
    }

    /// Written under a dot name `list` skips, then renamed over `name` in one step.
    async fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        std::fs::create_dir_all(&self.path).map_err(|e| failed("create", &self.path, e))?;
        let n = WRITES.fetch_add(1, Ordering::Relaxed);
        let temp = self
            .path
            .join(format!(".{name}.{}.{n}", std::process::id()));
        std::fs::write(&temp, bytes).map_err(|e| failed("write", &temp, e))?;
        let path = self.path.join(name);
        std::fs::rename(&temp, &path).map_err(|e| failed("rename onto", &path, e))
    }

    async fn delete(&self, name: &str) -> Result<(), String> {
        let path = self.path.join(name);
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(failed("delete", &path, e)),
            _ => Ok(()),
        }
    }

    async fn list(&self) -> Result<Vec<(String, u64)>, String> {
        let entries = match std::fs::read_dir(&self.path) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(failed("list", &self.path, e)),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let file = entry.metadata().is_ok_and(|m| m.is_file());
            if file && !name.starts_with('.') {
                out.push((name, entry.metadata().map_or(0, |m| m.len())));
            }
        }
        Ok(out)
    }
}
