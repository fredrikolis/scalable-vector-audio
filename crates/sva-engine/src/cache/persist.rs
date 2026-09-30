// Concern: node values on a backend, staged then committed: version, budget, eviction | Non-concern: the bytes' medium (a Backend) | IO: (key) -> Stored; samples -> staged; persist() -> commits

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard};

use sva_formula::{Codomain, Hash};
use sva_samples::{Buffer, Extent, Grid, Label};

use super::{codec, joined};

/// A node's samples, and what a render needs to read them in place of typing the node.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub key: Hash,
    pub segments: Vec<Buffer>,
    pub label: Label,
    pub width: u8,
    pub codomain: Codomain,
    pub rate: Option<u32>,
    pub grid: Grid,
    pub support: Extent,
    /// Flops it and every value under it cost when computed.
    pub priced: u128,
    /// The most seconds a read under it moved to land on a sample.
    pub moved: f64,
    /// A reader may take these samples in place of computing the node.
    pub readable: bool,
}

impl Stored {
    pub(crate) fn holds(&self, over: Extent) -> bool {
        let mut from = over.start;
        let mut parts: Vec<Extent> = self.segments.iter().map(Buffer::extent).collect();
        parts.sort_by_key(|e| e.start);
        for part in parts {
            if from >= over.end || part.start > from {
                break;
            }
            from = from.max(part.end);
        }
        from >= over.end
    }
}

/// One node's value at one rate and profile, named by its source.
pub(crate) fn node_key(identity: Hash, rate: u32, profile: &sva_samples::Profile) -> Hash {
    super::mixed(
        identity,
        &[
            u64::from(rate),
            profile.precision_bits as u64,
            profile.ceiling_hz.to_bits(),
            0x6e_6f_64_65_00_00_00_01,
        ],
    )
}

pub const DEFAULT_STORE_BYTES: u64 = 2 << 30;

pub const STORE_VERSION: &str = concat!(
    "sva-engine ",
    env!("CARGO_PKG_VERSION"),
    " build ",
    env!("SVA_ENGINE_BUILD"),
    " format 2"
);

pub const VERSION_NAME: &str = "version";

const RECENCY_NAME: &str = "recency";

const META: &str = "meta";

/// Named byte storage. A `put` is whole or absent: no reader ever sees half of one.
pub trait Backend: Sized {
    fn get(&self, name: &str) -> impl Future<Output = Result<Option<Vec<u8>>, String>>;
    fn put(&self, name: &str, bytes: &[u8]) -> impl Future<Output = Result<(), String>>;
    fn delete(&self, name: &str) -> impl Future<Output = Result<(), String>>;
    /// Every name held, with its size in bytes.
    fn list(&self) -> impl Future<Output = Result<Vec<(String, u64)>, String>>;
    /// An area of its own beside this one's names, which no other store writes and `list`
    /// never names.
    fn staging(&self) -> impl Future<Output = Result<Self, String>>;
    /// `name` moved from here into `to` in one step, over whatever `to` held under it.
    fn rename(&self, name: &str, to: &Self) -> impl Future<Output = Result<(), String>>;
}

#[derive(Default)]
struct Index {
    held: HashMap<Hash, (u64, u64)>,
    clock: u64,
    bytes: u64,
}

impl Index {
    fn touch(&mut self, key: Hash, bytes: u64) {
        self.clock += 1;
        let at = self.clock;
        if let Some((old, _)) = self.held.insert(key, (bytes, at)) {
            self.bytes -= old;
        }
        self.bytes += bytes;
    }

    fn read(&mut self, key: Hash) {
        self.clock += 1;
        let at = self.clock;
        if let Some((_, held)) = self.held.get_mut(&key) {
            *held = at;
        }
    }

    fn forget(&mut self, key: Hash) {
        if let Some((bytes, _)) = self.held.remove(&key) {
            self.bytes -= bytes;
        }
    }

    fn oldest_first(&self) -> Vec<Hash> {
        let mut keys: Vec<(u64, Hash)> = self.held.iter().map(|(k, (_, at))| (*at, *k)).collect();
        keys.sort_unstable();
        keys.into_iter().map(|(_, k)| k).collect()
    }
}

#[derive(Clone, Default)]
struct Staged {
    chunks: Vec<String>,
    meta: bool,
}

/// Node values under their keys. A render reads through it and spills what it computes to the
/// staging area; only `persist` changes the store, committing each staged value whole by
/// rename, least recently used going first past the budget.
pub struct Store<B> {
    backend: B,
    staging: B,
    max_bytes: u64,
    index: Mutex<Index>,
    staged: Mutex<BTreeMap<Hash, Staged>>,
    written: Mutex<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Persisted {
    pub written: usize,
    pub evicted: usize,
}

fn name_of(key: Hash) -> String {
    format!("{:016x}{:016x}", key.0, key.1)
}

fn key_of(name: &str) -> Option<Hash> {
    let hex = |s: &str| u64::from_str_radix(s, 16).ok();
    let valid = name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit());
    valid.then(|| Some(Hash(hex(&name[..16])?, hex(&name[16..])?)))?
}

fn locked<T>(held: &Mutex<T>) -> MutexGuard<'_, T> {
    held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl<B: Backend> Store<B> {
    /// A store another version wrote is emptied first.
    pub async fn open(backend: B, max_bytes: u64) -> Result<Store<B>, String> {
        let version = backend.get(VERSION_NAME).await?;
        if version.as_deref() != Some(STORE_VERSION.as_bytes()) {
            for (name, _) in backend.list().await? {
                if key_of(&name).is_some() || name == RECENCY_NAME || name == VERSION_NAME {
                    backend.delete(&name).await?;
                }
            }
            backend.put(VERSION_NAME, STORE_VERSION.as_bytes()).await?;
        }
        let order = backend.get(RECENCY_NAME).await?.unwrap_or_default();
        let order = String::from_utf8_lossy(&order);
        let rank: HashMap<&str, usize> = order.lines().enumerate().map(|(k, n)| (n, k)).collect();
        let mut held: Vec<(usize, Hash, u64)> = backend
            .list()
            .await?
            .into_iter()
            .filter_map(|(name, bytes)| {
                let at = rank.get(name.as_str()).map_or(0, |k| k + 1);
                Some((at, key_of(&name)?, bytes))
            })
            .collect();
        held.sort_unstable();
        let mut index = Index::default();
        for (_, key, bytes) in held {
            index.touch(key, bytes);
        }
        let staging = backend.staging().await?;
        Ok(Store {
            backend,
            staging,
            max_bytes,
            index: Mutex::new(index),
            staged: Mutex::new(BTreeMap::new()),
            written: Mutex::new(0),
        })
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub fn bytes(&self) -> u64 {
        locked(&self.index).bytes
    }

    pub fn holds(&self, key: Hash) -> bool {
        locked(&self.index).held.contains_key(&key)
    }

    /// Staged first. An entry that does not decode is a miss, left for `persist` to replace.
    pub(crate) async fn lookup(&self, key: Hash) -> Option<Stored> {
        if let Some(staged) = self.staged_value(key).await {
            return Some(staged);
        }
        if !self.holds(key) {
            return None;
        }
        let bytes = self.backend.get(&name_of(key)).await.ok()??;
        let found = codec::read_entry(&bytes).filter(|found| found.key == key)?;
        locked(&self.index).read(key);
        Some(found)
    }

    async fn staged_value(&self, key: Hash) -> Option<Stored> {
        let chunks = {
            let staged = locked(&self.staged);
            let held = staged.get(&key).filter(|staged| staged.meta)?;
            held.chunks.clone()
        };
        let meta = self.staging.get(&staged_name(key, META)).await.ok()??;
        let mut stored = codec::read_entry(&meta)?;
        for chunk in chunks {
            let bytes = self.staging.get(&chunk).await.ok()??;
            joined(&mut stored.segments, vec![codec::read_chunk(&bytes)?]);
        }
        Some(stored)
    }

    /// One run of `key`'s samples, out of memory and into the staging area.
    pub(crate) async fn stage(&self, key: Hash, samples: &Buffer) -> Result<(), String> {
        let n = {
            let mut written = locked(&self.written);
            *written += 1;
            *written
        };
        let name = staged_name(key, &n.to_string());
        self.staging.put(&name, &codec::chunk(samples)).await?;
        locked(&self.staged)
            .entry(key)
            .or_default()
            .chunks
            .push(name);
        Ok(())
    }

    /// `stored` holds no samples.
    pub(crate) async fn stage_meta(&self, key: Hash, stored: &Stored) -> Result<(), String> {
        let name = staged_name(key, META);
        self.staging.put(&name, &codec::entry(stored)).await?;
        locked(&self.staged).entry(key).or_default().meta = true;
        Ok(())
    }

    /// A staged value whose meta was never staged is dropped. Each key leaves the staging
    /// record only once its files have, so a write that fails leaves every later one staged.
    pub async fn persist(&self) -> Result<Persisted, String> {
        let mut done = Persisted::default();
        let keys: Vec<Hash> = locked(&self.staged).keys().copied().collect();
        for key in keys {
            let Some(held) = locked(&self.staged).get(&key).cloned() else {
                continue;
            };
            if let Some(bytes) = self.committed(key, &held).await? {
                locked(&self.index).touch(key, bytes);
                done.written += 1;
            }
            for chunk in &held.chunks {
                self.staging.delete(chunk).await?;
            }
            self.staging.delete(&staged_name(key, META)).await?;
            locked(&self.staged).remove(&key);
        }
        let oldest = locked(&self.index).oldest_first();
        for key in oldest {
            if self.bytes() <= self.max_bytes {
                break;
            }
            self.backend.delete(&name_of(key)).await?;
            locked(&self.index).forget(key);
            done.evicted += 1;
        }
        let order: String = locked(&self.index)
            .oldest_first()
            .into_iter()
            .map(|key| name_of(key) + "\n")
            .collect();
        self.backend.put(RECENCY_NAME, order.as_bytes()).await?;
        Ok(done)
    }

    /// `key`'s staged value moved whole into the store, and its size; none past the budget.
    async fn committed(&self, key: Hash, held: &Staged) -> Result<Option<u64>, String> {
        let whole = match held.meta {
            true => self.staged_value_of(key, held).await?,
            false => None,
        };
        let Some(stored) = whole.filter(|stored| !stored.segments.is_empty()) else {
            return Ok(None);
        };
        let bytes = codec::entry(&stored);
        if bytes.len() as u64 > self.max_bytes {
            return Ok(None);
        }
        let name = name_of(key);
        self.staging.put(&name, &bytes).await?;
        self.staging.rename(&name, &self.backend).await?;
        Ok(Some(bytes.len() as u64))
    }

    async fn staged_value_of(&self, key: Hash, held: &Staged) -> Result<Option<Stored>, String> {
        let Some(meta) = self.staging.get(&staged_name(key, META)).await? else {
            return Ok(None);
        };
        let Some(mut stored) = codec::read_entry(&meta) else {
            return Ok(None);
        };
        for chunk in &held.chunks {
            let bytes = self.staging.get(chunk).await?;
            match bytes.as_deref().and_then(codec::read_chunk) {
                Some(samples) => joined(&mut stored.segments, vec![samples]),
                None => return Ok(None),
            }
        }
        Ok(Some(stored))
    }
}

fn staged_name(key: Hash, part: &str) -> String {
    format!("{}.{part}", name_of(key))
}
