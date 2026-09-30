// Concern: node values on a backend, staged then committed: version, budget, eviction | Non-concern: the bytes' medium (a Backend) | IO: (key) -> Stored; samples -> staged; persist() -> commits

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use sva_formula::Hash;
use sva_samples::{Buffer, Extent};

use super::index::{Index, key_of, name_of};
use super::stored::Samples;
pub use super::stored::Stored;
use super::{codec, joined};

pub const DEFAULT_STORE_BYTES: u64 = 2 << 30;

/// Bumped by, and only by, a change to a stored value's bytes.
pub const STORE_FORMAT: u32 = 4;

fn version() -> String {
    format!("sva store format {STORE_FORMAT}")
}

pub const INDEX_NAME: &str = "index";

/// Kept beside entries by an older format.
const RETIRED: [&str; 2] = ["version", "recency"];

const META: &str = "meta";

/// Named bytes. A `put` is whole or absent: no reader sees half of one.
pub trait Backend: Sized {
    fn get(&self, name: &str) -> impl Future<Output = Result<Option<Vec<u8>>, String>>;
    /// At most `len` bytes from `from` on, fewer where the name ends sooner.
    fn get_range(
        &self,
        name: &str,
        from: u64,
        len: u64,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, String>>;
    fn put(&self, name: &str, bytes: &[u8]) -> impl Future<Output = Result<(), String>>;
    fn delete(&self, name: &str) -> impl Future<Output = Result<(), String>>;
    /// Each name, with its size in bytes.
    fn list(&self) -> impl Future<Output = Result<Vec<(String, u64)>, String>>;
    /// An area beside these names that no other store writes and `list` never names.
    fn staging(&self) -> impl Future<Output = Result<Self, String>>;
    /// `name` moved from here into `to` in one step, over whatever `to` held under it.
    fn rename(&self, name: &str, to: &Self) -> impl Future<Output = Result<(), String>>;
}

/// A store, or nothing, that a render looks a node up in and reads its samples from.
pub trait Through {
    fn lookup(&self, key: Hash) -> impl Future<Output = Option<Stored>>;
    /// Whole chunks holding `over`; `None` where they are gone or corrupt.
    fn read(&self, stored: &Stored, over: Extent) -> impl Future<Output = Option<Vec<Buffer>>>;
}

impl<B: Backend> Through for Store<B> {
    fn lookup(&self, key: Hash) -> impl Future<Output = Option<Stored>> {
        Store::lookup(self, key)
    }

    fn read(&self, stored: &Stored, over: Extent) -> impl Future<Output = Option<Vec<Buffer>>> {
        Store::read(self, stored, over)
    }
}

pub struct NoStore;

impl Through for NoStore {
    async fn lookup(&self, _: Hash) -> Option<Stored> {
        None
    }

    async fn read(&self, _: &Stored, _: Extent) -> Option<Vec<Buffer>> {
        None
    }
}

#[derive(Clone, Default)]
struct Staged {
    chunks: Vec<(String, Extent)>,
    meta: bool,
}

/// Node values under their keys: a render stages, and only `persist` commits each value whole
/// by rename, evicting the least recently used past the budget.
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

fn locked<T>(held: &Mutex<T>) -> MutexGuard<'_, T> {
    held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl<B: Backend> Store<B> {
    /// A store of another format is emptied first.
    pub async fn open(backend: B, max_bytes: u64) -> Result<Store<B>, String> {
        let version = version();
        let found = backend.get(INDEX_NAME).await?;
        let index = match found.and_then(|text| Index::read(&text, &version)) {
            Some(index) => index,
            None => {
                for (name, _) in backend.list().await? {
                    if key_of(&name).is_some() || RETIRED.contains(&name.as_str()) {
                        backend.delete(&name).await?;
                    }
                }
                let empty = Index::default();
                backend
                    .put(INDEX_NAME, empty.text(&version).as_bytes())
                    .await?;
                empty
            }
        };
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

    /// Staged first; past the index, the backend, for another store's commits. Reads a header,
    /// never a sample.
    pub(crate) async fn lookup(&self, key: Hash) -> Option<Stored> {
        if let Some(staged) = self.staged_value(key).await {
            return Some(staged);
        }
        let name = name_of(key);
        let Some(mut bytes) = self.backend.get_range(&name, 0, 8).await.ok()? else {
            locked(&self.index).forget(key);
            return None;
        };
        let rest = codec::head_len(&bytes)? - 8;
        bytes.extend(self.backend.get_range(&name, 8, rest as u64).await.ok()??);
        let (found, len) = codec::read_head(&bytes, key).filter(|(found, _)| found.key == key)?;
        let mut index = locked(&self.index);
        match index.held.contains_key(&key) {
            true => index.used(key),
            false => index.touch(key, len),
        }
        Some(found)
    }

    async fn staged_value(&self, key: Hash) -> Option<Stored> {
        let chunks = {
            let staged = locked(&self.staged);
            let held = staged.get(&key).filter(|staged| staged.meta)?;
            held.chunks.clone()
        };
        let meta = self.staging.get(&staged_name(key, META)).await.ok()??;
        let (mut stored, _) = codec::read_head(&meta, key)?;
        stored.samples = Samples::Staged(chunks);
        Some(stored)
    }

    pub(crate) async fn read(&self, stored: &Stored, over: Extent) -> Option<Vec<Buffer>> {
        let mut out = Vec::new();
        match &stored.samples {
            Samples::None => {}
            Samples::Entry { file, runs } => {
                for run in runs {
                    let met = run.extent().intersect(over);
                    if met.is_empty() {
                        continue;
                    }
                    let chunk = codec::CHUNK as i64;
                    let from = ((met.start - run.start) / chunk) as usize;
                    let to = (met.end - run.start).div_euclid(chunk) as usize;
                    let to = to + usize::from((met.end - run.start) % chunk != 0);
                    let (at, len) = codec::span_of(run, from, to);
                    let bytes = self.backend.get_range(&name_of(*file), at, len).await;
                    out.push(codec::read_chunks(&bytes.ok()??, run, from, to)?);
                }
            }
            Samples::Staged(chunks) => {
                for (name, e) in chunks {
                    if !e.intersect(over).is_empty() {
                        let bytes = self.staging.get(name).await.ok()??;
                        out.push(codec::read_chunk(&bytes)?);
                    }
                }
            }
        }
        Some(out)
    }

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
            .push((name, samples.extent()));
        Ok(())
    }

    /// `stored` holds no samples.
    pub(crate) async fn stage_meta(&self, key: Hash, stored: &Stored) -> Result<(), String> {
        let name = staged_name(key, META);
        self.staging.put(&name, &codec::entry(stored, &[])).await?;
        locked(&self.staged).entry(key).or_default().meta = true;
        Ok(())
    }

    /// A value staged without its meta is dropped. A key leaves the staging record only once
    /// its files have, so a failed write leaves every later one staged.
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
            for (chunk, _) in &held.chunks {
                self.staging.delete(chunk).await?;
            }
            self.staging.delete(&staged_name(key, META)).await?;
            locked(&self.staged).remove(&key);
        }
        let listed = self.backend.list().await?;
        let listed = listed
            .into_iter()
            .filter_map(|(name, bytes)| Some((key_of(&name)?, bytes)));
        locked(&self.index).sync(listed.collect());
        let oldest = locked(&self.index).oldest_first();
        for key in oldest {
            if self.bytes() <= self.max_bytes {
                break;
            }
            self.backend.delete(&name_of(key)).await?;
            locked(&self.index).forget(key);
            done.evicted += 1;
        }
        let text = locked(&self.index).text(&version());
        self.backend.put(INDEX_NAME, text.as_bytes()).await?;
        Ok(done)
    }

    /// `key`'s staged value moved whole into the store, and its size; none past budget.
    async fn committed(&self, key: Hash, held: &Staged) -> Result<Option<u64>, String> {
        let whole = match held.meta {
            true => self.staged_value_of(key, held).await?,
            false => None,
        };
        let Some((stored, runs)) = whole.filter(|(_, runs)| !runs.is_empty()) else {
            return Ok(None);
        };
        let bytes = codec::entry(&stored, &runs);
        if bytes.len() as u64 > self.max_bytes {
            return Ok(None);
        }
        let name = name_of(key);
        self.staging.put(&name, &bytes).await?;
        self.staging.rename(&name, &self.backend).await?;
        Ok(Some(bytes.len() as u64))
    }

    async fn staged_value_of(
        &self,
        key: Hash,
        held: &Staged,
    ) -> Result<Option<(Stored, Vec<Buffer>)>, String> {
        let Some(meta) = self.staging.get(&staged_name(key, META)).await? else {
            return Ok(None);
        };
        let Some((stored, _)) = codec::read_head(&meta, key) else {
            return Ok(None);
        };
        let mut runs = Vec::new();
        for (chunk, _) in &held.chunks {
            let bytes = self.staging.get(chunk).await?;
            match bytes.as_deref().and_then(codec::read_chunk) {
                Some(samples) => joined(&mut runs, vec![samples]),
                None => return Ok(None),
            }
        }
        Ok(Some((stored, runs)))
    }
}

fn staged_name(key: Hash, part: &str) -> String {
    format!("{}.{part}", name_of(key))
}
