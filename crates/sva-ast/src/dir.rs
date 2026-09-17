// Concern: answers node texts off a directory, the one place this crate opens a file | Non-concern: what a text means (graph.rs), holding one in memory (source.rs) | IO: (dir) -> a Listing, node texts

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};

use crate::source::{Listing, Source};

/// Read on demand: `get` opens exactly the files a loader asks for.
pub struct Dir {
    root: PathBuf,
}

impl Dir {
    pub fn at(root: impl Into<PathBuf>) -> Dir {
        Dir { root: root.into() }
    }
}

impl Source for Dir {
    fn paths(&self) -> Result<Listing, String> {
        let mut out = Listing::default();
        walk(&self.root, &self.root, &mut out)?;
        out.found.sort();
        out.unreadable.sort();
        Ok(out)
    }

    /// A node is its file's whole content, so anything that is not text is not a node.
    fn get(&self, path: &str) -> Result<Option<Cow<'_, str>>, String> {
        let full = self.root.join(path);
        match classify(&full) {
            Ok(Kind::Dir) => return Err(format!("{path} is a directory, so it cannot be a node")),
            Ok(Kind::Other) => {
                return Err(format!(
                    "{path} is not a regular file (FIFO, socket, or device), so it cannot be a node"
                ));
            }
            // A stat error here falls through to read_to_string, which reports it identically.
            Ok(Kind::File) | Err(_) => {}
        }
        match fs::read_to_string(&full) {
            Ok(text) => Ok(Some(Cow::Owned(text))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Err(format!(
                "{path} is not text, so it cannot be a node; a composition directory holds only \
                 node files, and a rendering written into one is read as a node"
            )),
            Err(e) => Err(format!("could not read {path}: {e}")),
        }
    }

    fn name(&self) -> String {
        self.root.display().to_string()
    }
}

enum Kind {
    Dir,
    File,
    Other,
}

fn classify(path: &Path) -> Result<Kind, std::io::Error> {
    let meta = fs::metadata(path)?;
    Ok(if meta.is_dir() {
        Kind::Dir
    } else if meta.is_file() {
        Kind::File
    } else {
        Kind::Other
    })
}

fn hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

fn walk(root: &Path, dir: &Path, out: &mut Listing) -> Result<(), String> {
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("could not read directory {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("could not read directory entry: {e}"))?;
        let path = entry.path();
        let named = || {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            rel.to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        };
        match classify(&path) {
            // A dot directory belongs to a tool; nothing under it is ever opened.
            Ok(Kind::Dir) if hidden(&path) => {}
            Ok(Kind::Dir) => walk(root, &path, out)?,
            Ok(Kind::File) => out.found.push(named()),
            // Opening one would block or answer bytes no node can be; the loader reports it.
            Ok(Kind::Other) => out.unreadable.push(named()),
            // Found anyway: get() re-stats, so a real error still surfaces there; a raced deletion is treated as missing, as any absent path already is.
            Err(_) => out.found.push(named()),
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn walk_and_get_skip_a_fifo_instead_of_blocking() {
        let root = std::env::temp_dir().join(format!("sva-ast-dir-fifo-{:x}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("node.txt"), "a node").unwrap();
        let fifo = root.join("blocked.fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success(), "mkfifo failed");

        let (tx, rx) = mpsc::channel();
        let probe_root = root.clone();
        std::thread::spawn(move || {
            let dir = Dir::at(&probe_root);
            let paths = dir.paths();
            let fifo_get = dir
                .get("blocked.fifo")
                .map(|opt| opt.map(|c| c.into_owned()));
            let _ = tx.send((paths, fifo_get));
        });
        let (paths, fifo_get) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("walk/get hung on a FIFO — did the fix regress?");

        let paths = paths.unwrap();
        assert_eq!(paths.unreadable, ["blocked.fifo"]);
        assert_eq!(paths.found, ["node.txt"]);
        assert!(fifo_get.is_err());

        let dir = Dir::at(&root);
        assert_eq!(dir.get("node.txt").unwrap().as_deref(), Some("a node"));

        let _ = fs::remove_dir_all(&root);
    }
}
