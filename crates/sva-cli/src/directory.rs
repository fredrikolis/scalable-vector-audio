// Concern: named bytes under one filesystem path, each write whole or absent | Non-concern: what the bytes mean, the budget (sva-engine) | IO: (name[, bytes]) -> bytes, names, or why not

use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sva_core::Backend;

/// One implementation for a disk directory and `/dev/shm` alike: both are paths.
pub struct Directory {
    pub path: PathBuf,
    /// A staging area's lock, held for as long as it is in use.
    _lock: Option<File>,
}

impl Directory {
    pub fn at(path: PathBuf) -> Directory {
        Directory { path, _lock: None }
    }

    /// Every staging area whose lock no process holds, removed; its lock file stays, so a
    /// process never waits on a lock file another is deleting.
    fn swept(&self) -> Result<(), String> {
        let Ok(entries) = std::fs::read_dir(&self.path) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let dir = entry.metadata().is_ok_and(|m| m.is_dir());
            if !dir || !name.starts_with(STAGING) {
                continue;
            }
            let lock = self.path.join(format!("{name}.lock"));
            let free = match File::options().write(true).open(&lock) {
                Ok(file) => file.try_lock().is_ok(),
                Err(_) => true,
            };
            if free {
                let path = entry.path();
                std::fs::remove_dir_all(&path).map_err(|e| failed("remove", &path, e))?;
            }
        }
        Ok(())
    }
}

const STAGING: &str = ".staging-";

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

    async fn get_range(&self, name: &str, from: u64, len: u64) -> Result<Option<Vec<u8>>, String> {
        let path = self.path.join(name);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(failed("open", &path, e)),
        };
        let mut out = Vec::new();
        file.seek(SeekFrom::Start(from))
            .and_then(|_| file.take(len).read_to_end(&mut out))
            .map_err(|e| failed("read", &path, e))?;
        Ok(Some(out))
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

    /// `.staging-<pid>`, locked before it exists, so a sweep never removes it while it runs.
    async fn staging(&self) -> Result<Directory, String> {
        std::fs::create_dir_all(&self.path).map_err(|e| failed("create", &self.path, e))?;
        let name = format!("{STAGING}{}", std::process::id());
        let lock = self.path.join(format!("{name}.lock"));
        let held = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock)
            .map_err(|e| failed("open", &lock, e))?;
        held.lock().map_err(|e| failed("lock", &lock, e))?;
        self.swept()?;
        Ok(Directory {
            path: self.path.join(name),
            _lock: Some(held),
        })
    }

    async fn rename(&self, name: &str, to: &Directory) -> Result<(), String> {
        std::fs::create_dir_all(&to.path).map_err(|e| failed("create", &to.path, e))?;
        let (from, onto) = (self.path.join(name), to.path.join(name));
        std::fs::rename(&from, &onto).map_err(|e| failed("rename onto", &onto, e))
    }
}
