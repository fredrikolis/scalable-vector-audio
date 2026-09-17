// Concern: keeps rendered buffers on disk under their content hash, across processes | Non-concern: what a store is for (cache.rs), computing a hash | IO: (Hash) -> a buffer + traces

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use sva_formula::filter::Shape;
use sva_samples::Buffer;
use sva_samples::{AutomationFrame, FilterTrace, Label};

use sva_formula::Hash;

use super::evict;
use super::{Cache, Entry, Expected, Payload, PayloadKind};

/// Recompute wins a tie; `tests/stores.rs` prices a round trip against it.
pub const IO_NANOS_PER_BYTE: u64 = 3;

/// Below this, nothing is worth a file: an inode is not free.
pub const MIN_COST: Duration = Duration::from_millis(1);
/// A 160 s master's node set is about 6 GB; less than that is no cache at all.
pub const DEFAULT_MAX_BYTES: u64 = 16 << 30;

/// A tag after the magic makes an older file a miss rather than a misread.
const MAGIC_TIME: &[u8; 4] = b"RBC6";
const TAG_SAMPLES: u8 = 0;

/// Two threads can reach one key at once, so a per-process temp name is not enough.
static WRITE: AtomicU64 = AtomicU64::new(0);

pub struct DiskCache {
    dir: PathBuf,
    max_bytes: u64,
    store_everything: bool,
    held: AtomicU64,
    evicted: AtomicU64,
    faults: AtomicU64,
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
        }
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

    /// A render that reuses nothing and evicted a lot has outgrown its budget, and nothing
    /// else in the report says so.
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
        match decode(&bytes, node, rate, width, samples) {
            Some(entry) => {
                touch(&path);
                Some(entry)
            }
            None => {
                self.faults.fetch_add(1, Ordering::Relaxed);
                let _ = fs::remove_file(&path);
                None
            }
        }
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
        if fs::write(&temp, encode(buffer, traces, label)).is_err() {
            self.faults.fetch_add(1, Ordering::Relaxed);
            let _ = fs::remove_file(&temp);
            return;
        }
        if fs::rename(&temp, &path).is_err() {
            self.faults.fetch_add(1, Ordering::Relaxed);
            let _ = fs::remove_file(&temp);
        }
    }

    /// The read time is the mtime `load` refreshes.
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

pub(super) struct Writer(pub Vec<u8>);

impl Writer {
    pub(super) fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub(super) fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
}

fn encode(buffer: &Buffer, traces: &[FilterTrace], label: Option<&Label>) -> Vec<u8> {
    encode_time(buffer, traces, label)
}

fn write_traces(w: &mut Writer, traces: &[FilterTrace]) {
    w.u32(traces.len() as u32);
    for trace in traces {
        w.u32(trace.site as u32);
        w.u32(trace.channel.map_or(u32::MAX, |c| c as u32));
        w.0.push(u8::from(trace.clamped));
        w.u32(trace.shape.len() as u32);
        w.0.extend_from_slice(trace.shape.as_bytes());
        w.f64(trace.trace_secs);
        w.u32(trace.frames.len() as u32);
        for f in &trace.frames {
            w.f64(f.t_secs);
            w.f64(f.cutoff);
            w.f64(f.q);
            w.f64(f.gain_db);
        }
    }
}

fn encode_time(buffer: &Buffer, traces: &[FilterTrace], label: Option<&Label>) -> Vec<u8> {
    let mut w = Writer(Vec::with_capacity(buffer.len() * buffer.width * 8 + 64));
    w.0.extend_from_slice(MAGIC_TIME);
    w.0.push(TAG_SAMPLES);
    w.u32(buffer.rate);
    w.u64(buffer.len() as u64);
    w.u32(buffer.width as u32);
    write_traces(&mut w, traces);
    super::label::write(&mut w, label);
    for c in 0..buffer.width {
        for &s in buffer.plane(c) {
            w.0.extend_from_slice(&s.to_le_bytes());
        }
    }
    w.0
}

pub(super) struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    pub(super) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }
    pub(super) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    pub(super) fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

fn read_f64s(r: &mut Reader, n: usize) -> Option<Vec<f64>> {
    Some(
        r.take(n * 8)?
            .chunks_exact(8)
            .map(|c| f64::from_le_bytes(c.try_into().expect("chunks_exact(8)")))
            .collect(),
    )
}

fn read_traces(r: &mut Reader, node: &str) -> Option<Vec<FilterTrace>> {
    let mut traces = Vec::new();
    for _ in 0..r.u32()? {
        let site = r.u32()? as usize;
        let channel = match r.u32()? {
            u32::MAX => None,
            c => Some(c as usize),
        };
        let clamped = r.take(1)?[0] == 1;
        let name_len = r.u32()? as usize;
        let shape = Shape::from_name(std::str::from_utf8(r.take(name_len)?).ok()?)?.name();
        let trace_secs = r.f64()?;
        let frame_count = r.u32()? as usize;
        let mut frames = Vec::with_capacity(frame_count.min(1 << 20));
        for _ in 0..frame_count {
            frames.push(AutomationFrame {
                t_secs: r.f64()?,
                cutoff: r.f64()?,
                q: r.f64()?,
                gain_db: r.f64()?,
            });
        }
        traces.push(FilterTrace {
            node: node.to_string(),
            site,
            channel,
            shape,
            clamped,
            trace_secs,
            frames,
        });
    }
    Some(traces)
}

/// Every field is checked against what the caller asked for, not merely parsed: the hash
/// already rules out a mismatch, so one here means the hash's domain is wrong and the only
/// safe answer is to re-render.
fn decode(bytes: &[u8], node: &str, rate: u32, width: usize, samples: usize) -> Option<Entry> {
    let mut r = Reader(bytes);
    if r.take(4)? != MAGIC_TIME || r.take(1)?[0] != TAG_SAMPLES {
        return None;
    }
    decode_time(&mut r, node, rate, width, samples)
}

fn decode_time(
    r: &mut Reader,
    node: &str,
    sample_rate: u32,
    width: usize,
    samples: usize,
) -> Option<Entry> {
    if r.u32()? != sample_rate || r.u64()? != samples as u64 || r.u32()? != width as u32 {
        return None;
    }
    let traces = read_traces(r, node)?;
    let label = super::label::read(r)?;
    let planes: Vec<Vec<f64>> = (0..width)
        .map(|_| read_f64s(r, samples))
        .collect::<Option<_>>()?;
    if !r.0.is_empty() {
        return None;
    }
    Some(Entry {
        payload: Payload::Samples(Box::new(Buffer::of_planes(sample_rate, planes))),
        traces,
        label,
    })
}
