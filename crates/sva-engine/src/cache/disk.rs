// Concern: keeps rendered buffers on disk under their content hash, across processes | Non-concern: what a store is for (cache.rs), computing a hash | IO: (Hash) -> a buffer + traces

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use sva_samples::{FilterTrace, Label};

use sva_formula::Hash;

use super::entry_bytes::{self, RawF64, SampleCodec, Unread};
use super::evict;
use super::{Cache, Entry, Expected, Payload, PayloadKind};

/// Recompute wins a tie; `examples/io_price.rs` measures a round trip against it.
pub const IO_NANOS_PER_BYTE: u64 = 3;

/// Below this, nothing is worth a file: an inode is not free.
pub const MIN_COST: Duration = Duration::from_millis(1);
/// A 160 s master's node set is about 6 GB; less than that is no cache at all.
pub const DEFAULT_MAX_BYTES: u64 = 16 << 30;

/// Two threads can reach one key at once, so a per-process temp name is not enough.
static WRITE: AtomicU64 = AtomicU64::new(0);

pub struct DiskCache {
    dir: PathBuf,
    max_bytes: u64,
    store_everything: bool,
    held: AtomicU64,
    evicted: AtomicU64,
    faults: AtomicU64,
    codec: Box<dyn SampleCodec>,
}

impl DiskCache {
    /// `$SVA_CACHE`, else the XDG cache home. Never under `target/`: `cargo clean` would throw
    /// away hours of rendering, and an installed binary has no build directory to write into.
    pub fn discover() -> Option<DiskCache> {
        let dir = match std::env::var_os("SVA_CACHE") {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => match std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
                Some(v) => PathBuf::from(v).join("sva"),
                None => PathBuf::from(std::env::var_os("HOME")?)
                    .join(".cache")
                    .join("sva"),
            },
        };
        Some(DiskCache::at(dir))
    }

    pub fn at(dir: impl Into<PathBuf>) -> DiskCache {
        let max_bytes = std::env::var("SVA_CACHE_MAX_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_BYTES);
        DiskCache::bounded(dir, max_bytes)
    }

    pub fn bounded(dir: impl Into<PathBuf>, max_bytes: u64) -> DiskCache {
        DiskCache {
            dir: dir.into(),
            max_bytes,
            store_everything: false,
            held: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
            faults: AtomicU64::new(0),
            codec: Box::new(RawF64),
        }
    }

    pub fn coded(mut self, codec: Box<dyn SampleCodec>) -> DiskCache {
        self.codec = codec;
        self
    }

    /// The threshold is policy: a caller measuring reuse itself wants every node stored.
    pub fn storing_everything(mut self) -> DiskCache {
        self.store_everything = true;
        self
    }

    fn path_of(&self, key: Hash) -> PathBuf {
        let hex = key.to_string();
        self.dir.join(&hex[..2]).join(format!("{hex}.rbc"))
    }
}

impl Cache for DiskCache {
    fn dir(&self) -> Option<&Path> {
        Some(&self.dir)
    }

    fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    fn held_bytes(&self) -> u64 {
        self.held.load(Ordering::Relaxed)
    }

    fn evicted_bytes(&self) -> u64 {
        self.evicted.load(Ordering::Relaxed)
    }

    fn faults(&self) -> u64 {
        self.faults.load(Ordering::Relaxed)
    }

    fn holds(&self, key: Hash) -> bool {
        self.path_of(key).is_file()
    }

    fn load(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        let Expected::Samples {
            rate,
            width,
            samples,
        } = expected
        else {
            return None;
        };
        let path = self.path_of(key);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            // Absent is a cold miss; anything else is a store that could not answer.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(_) => {
                self.faults.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        match entry_bytes::decode(&bytes, node, (rate, width, samples), self.codec.as_ref()) {
            Ok(entry) => {
                touch(&path);
                Some(entry)
            }
            Err(Unread::OtherCodec) => None,
            Err(Unread::Damaged) => {
                self.faults.fetch_add(1, Ordering::Relaxed);
                let _ = fs::remove_file(&path);
                None
            }
        }
    }

    fn peek(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        let Expected::Samples {
            rate,
            width,
            samples,
        } = expected
        else {
            return None;
        };
        let bytes = fs::read(self.path_of(key)).ok()?;
        entry_bytes::decode(&bytes, node, (rate, width, samples), self.codec.as_ref()).ok()
    }

    /// Only samples have an encoding here; neither other payload is offered a file.
    fn worth_storing(&self, cost: Duration, bytes: usize, kind: PayloadKind) -> bool {
        kind == PayloadKind::Samples
            && (self.store_everything
                || (cost >= MIN_COST
                    && cost.as_nanos() > u128::from(bytes as u64) * u128::from(IO_NANOS_PER_BYTE)))
    }

    /// Renamed into place: a racing reader sees one whole entry or the other.
    fn store(&self, key: Hash, payload: &Payload, traces: &[FilterTrace], label: Option<&Label>) {
        let Payload::Samples(buffer) = payload else {
            unreachable!("worth_storing offers this store nothing but samples")
        };
        let path = self.path_of(key);
        let Some(shard) = path.parent() else {
            return;
        };
        if fs::create_dir_all(shard).is_err() {
            self.faults.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let temp = shard.join(format!(
            "{key}.{}.{}.tmp",
            std::process::id(),
            WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        if fs::write(
            &temp,
            entry_bytes::encode(buffer, traces, label, self.codec.as_ref()),
        )
        .is_err()
        {
            self.faults.fetch_add(1, Ordering::Relaxed);
            let _ = fs::remove_file(&temp);
            return;
        }
        if fs::rename(&temp, &path).is_err() {
            self.faults.fetch_add(1, Ordering::Relaxed);
            let _ = fs::remove_file(&temp);
        }
    }

    fn sweep(&self) {
        let mut entries: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        for shard in fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            for file in fs::read_dir(shard.path()).into_iter().flatten().flatten() {
                let Ok(meta) = file.metadata() else { continue };
                let when = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                entries.push((when, meta.len(), file.path()));
            }
        }
        let swept = evict::to_cap(entries, self.max_bytes, |path| {
            fs::remove_file(path).is_ok()
        });
        self.held.store(swept.held, Ordering::Relaxed);
        self.evicted.store(swept.evicted, Ordering::Relaxed);
    }
}

fn touch(path: &Path) {
    if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}
