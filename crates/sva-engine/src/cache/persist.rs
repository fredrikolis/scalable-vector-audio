// Concern: node values on a backend beneath memory, staged then committed: version, budget, eviction | Non-concern: the bytes' medium (a Backend) | IO: (key) -> Header; samples -> staged; persist()

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use sva_formula::{Hash, TABLE_VERSION};
use sva_samples::{Buffer, Extent};

use super::codec::{self, STORE_FORMAT};
use super::index::{Index, key_of, name_of};
use super::joined;
use super::stored::{Header, Samples};

pub const DEFAULT_STORE_BYTES: u64 = 2 << 30;

fn version() -> String {
    format!("sva store format {STORE_FORMAT} table {TABLE_VERSION}")
}

pub const INDEX_NAME: &str = "index";

const META: &str = "meta";

/// Named bytes. A `put` is whole or absent: no reader sees half of one.
pub trait Backend: Sized {
    type Lock;
    /// The one exclusive lock over these names, shared by every store over them.
    fn lock(&self) -> impl Future<Output = Result<Self::Lock, String>>;
    fn get(&self, name: &str) -> impl Future<Output = Result<Option<Vec<u8>>, String>>;
    /// At most `len` bytes from `from` on, fewer where the name ends sooner.
    fn get_range(
        &self,
        name: &str,
        from: u64,
        len: u64,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, String>>;
    fn put(&self, name: &str, bytes: &[u8]) -> impl Future<Output = Result<(), String>>;
    /// False where another holder has `name` open, so it stays.
    fn delete(&self, name: &str) -> impl Future<Output = Result<bool, String>>;
    /// Each name, with its size in bytes.
    fn list(&self) -> impl Future<Output = Result<Vec<(String, u64)>, String>>;
    /// An area beside these names that no other store writes and `list` never names.
    fn staging(&self) -> impl Future<Output = Result<Self, String>>;
    /// `name` moved from here into `to` in one step, over whatever `to` held under it; as
    /// `delete`, false where either is open.
    fn rename(&self, name: &str, to: &Self) -> impl Future<Output = Result<bool, String>>;
}

#[derive(Clone, Default)]
struct Staged {
    chunks: Vec<(String, Extent)>,
    meta: bool,
}

/// Node values under their keys, beneath the memory tier alone: it stages, and only `persist`
/// commits each value whole by rename, evicting the least recently used past the budget. Every
/// change to the backend's names runs under its lock; lookups and reads never wait on it.
pub struct Store<B> {
    backend: B,
    staging: B,
    max_bytes: u64,
    index: Mutex<Index>,
    staged: Mutex<BTreeMap<Hash, Staged>>,
    written: Mutex<u64>,
}

enum Prepared {
    Ready { bytes: u64 },
    Dropped,
    Refused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Persisted {
    pub written: usize,
    pub evicted: usize,
    /// Values larger than the whole budget.
    pub refused: usize,
}

fn locked<T>(held: &Mutex<T>) -> MutexGuard<'_, T> {
    held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl<B: Backend> Store<B> {
    /// Over another format's index, a fresh one: what that format left is neither read nor
    /// removed, nor counted against the budget.
    pub async fn open(backend: B, max_bytes: u64) -> Result<Store<B>, String> {
        let version = version();
        let held = backend.lock().await?;
        let found = backend.get(INDEX_NAME).await?;
        let index = match found.and_then(|text| Index::read(&text, &version)) {
            Some(index) => index,
            None => {
                let empty = Index::default();
                backend
                    .put(INDEX_NAME, empty.text(&version).as_bytes())
                    .await?;
                empty
            }
        };
        let staging = backend.staging().await?;
        drop(held);
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

    pub(crate) fn used(&self, key: Hash) {
        locked(&self.index).used(key);
    }

    /// Reads headers, never a sample; one standing for another's samples reads that one's.
    pub(crate) async fn lookup(&self, key: Hash) -> Option<Header> {
        let found = self.written(key).await?;
        let &Samples::Of { key: of, by } = found.samples() else {
            return Some(found);
        };
        let samples = match self.written(of).await?.into_parts().1 {
            Samples::Entry { file, runs, .. } => Samples::Entry {
                file,
                runs,
                shift: -by,
            },
            Samples::Staged { chunks, .. } => Samples::Staged { chunks, shift: -by },
            _ => return None,
        };
        Some(Header::new(found.into_parts().0, samples))
    }

    /// Staged first; past the index, the backend, for another store's commits.
    async fn written(&self, key: Hash) -> Option<Header> {
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
        let (found, len) =
            codec::read_head(&bytes, key).filter(|(found, _)| found.stored().key == key)?;
        let mut index = locked(&self.index);
        match index.held.contains_key(&key) {
            true => index.used(key),
            false => index.touch(key, len),
        }
        Some(found)
    }

    async fn staged_value(&self, key: Hash) -> Option<Header> {
        let chunks = {
            let staged = locked(&self.staged);
            let held = staged.get(&key).filter(|staged| staged.meta)?;
            held.chunks.clone()
        };
        let meta = self.staging.get(&staged_name(key, META)).await.ok()??;
        let (found, _) = codec::read_head(&meta, key)?;
        Some(match found.refers() {
            true => found,
            false => Header::new(found.into_parts().0, Samples::Staged { chunks, shift: 0 }),
        })
    }

    /// Whole chunks holding `over`; `None` where they are gone or corrupt.
    pub(crate) async fn read(&self, head: &Header, over: Extent) -> Option<Vec<Buffer>> {
        let mut out = Vec::new();
        match head.samples() {
            Samples::None | Samples::Of { .. } => {}
            Samples::Entry { file, runs, shift } => {
                for run in runs {
                    let met = run.extent().intersect(over.shifted(-shift));
                    if met.is_empty() {
                        continue;
                    }
                    let chunk = codec::CHUNK as i64;
                    let from = ((met.start - run.start) / chunk) as usize;
                    let to = (met.end - run.start).div_euclid(chunk) as usize;
                    let to = to + usize::from((met.end - run.start) % chunk != 0);
                    let (at, len) = codec::span_of(run, from, to);
                    let bytes = self.backend.get_range(&name_of(*file), at, len).await;
                    let mut samples = codec::read_chunks(&bytes.ok()??, run, from, to)?;
                    samples.start += shift;
                    out.push(samples);
                }
            }
            Samples::Staged { chunks, shift } => {
                for (name, e) in chunks {
                    if !e.intersect(over.shifted(-shift)).is_empty() {
                        let bytes = self.staging.get(name).await.ok()??;
                        let mut samples = codec::read_chunk(&bytes)?;
                        samples.start += shift;
                        out.push(samples);
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

    pub(crate) async fn stage_meta(&self, key: Hash, head: &Header) -> Result<(), String> {
        let name = staged_name(key, META);
        self.staging.put(&name, &codec::entry(head, &[])).await?;
        locked(&self.staged).entry(key).or_default().meta = true;
        Ok(())
    }

    /// Free while nothing is staged or used since and the budget holds. The index file is
    /// every store's account and names each value before it moves: one cut short leaves a
    /// gone entry named, never one unnamed. A value staged without its meta is dropped; one
    /// failed or over an open entry stays staged.
    pub(crate) async fn persist(&self) -> Result<Persisted, String> {
        let idle = {
            let index = locked(&self.index);
            index.bytes <= self.max_bytes && index.unsynced().is_empty()
        };
        if idle && locked(&self.staged).is_empty() {
            return Ok(Persisted::default());
        }
        let _held = self.backend.lock().await?;
        let (mut index, seen) = self.current().await?;
        let mut done = Persisted::default();
        let mut moving = Vec::new();
        let keys: Vec<Hash> = locked(&self.staged).keys().copied().collect();
        for key in keys {
            let Some(held) = locked(&self.staged).get(&key).cloned() else {
                continue;
            };
            match self.prepared(key, &held).await? {
                Prepared::Ready { bytes } => {
                    moving.push((key, held, bytes));
                    continue;
                }
                Prepared::Dropped => {}
                Prepared::Refused => done.refused += 1,
            }
            self.unstaged(key, &held).await?;
        }
        if !moving.is_empty() {
            let mut naming = index.clone();
            for (key, _, bytes) in &moving {
                naming.touch(*key, *bytes);
            }
            let text = naming.text(&version());
            self.backend.put(INDEX_NAME, text.as_bytes()).await?;
        }
        for (key, held, bytes) in moving {
            if !self.staging.rename(&name_of(key), &self.backend).await? {
                continue;
            }
            index.touch(key, bytes);
            done.written += 1;
            self.unstaged(key, &held).await?;
        }
        for key in index.oldest_first() {
            if index.bytes <= self.max_bytes {
                break;
            }
            if self.backend.delete(&name_of(key)).await? {
                index.forget(key);
                done.evicted += 1;
            }
        }
        self.backend
            .put(INDEX_NAME, index.text(&version()).as_bytes())
            .await?;
        index.synced();
        let mut local = locked(&self.index);
        for key in local.used_since(seen) {
            index.used(key);
        }
        *local = index;
        Ok(done)
    }

    /// The index file, or else the directory listed, with this store's uses since; its clock.
    async fn current(&self) -> Result<(Index, u64), String> {
        let found = self.backend.get(INDEX_NAME).await?;
        let mut index = match found.and_then(|text| Index::read(&text, &version())) {
            Some(index) => index,
            None => {
                let listed = self.backend.list().await?;
                let listed = listed
                    .into_iter()
                    .filter_map(|(name, bytes)| Some((key_of(&name)?, bytes)));
                Index::listed(listed.collect(), &locked(&self.index))
            }
        };
        let local = locked(&self.index);
        for key in local.unsynced() {
            index.used(key);
        }
        Ok((index, local.clock()))
    }

    async fn unstaged(&self, key: Hash, held: &Staged) -> Result<(), String> {
        for (chunk, _) in &held.chunks {
            self.staging.delete(chunk).await?;
        }
        self.staging.delete(&staged_name(key, META)).await?;
        locked(&self.staged).remove(&key);
        Ok(())
    }

    /// `key`'s staged value joined with its entry's, whole in staging under the entry's name.
    async fn prepared(&self, key: Hash, held: &Staged) -> Result<Prepared, String> {
        let whole = match held.meta {
            true => self.staged_value_of(key, held).await?,
            false => None,
        };
        let whole = whole.filter(|(head, runs)| !runs.is_empty() || head.refers());
        let Some((head, mut runs)) = whole else {
            return Ok(Prepared::Dropped);
        };
        let name = name_of(key);
        if !head.refers() {
            let held = self.backend.get(&name).await?;
            if let Some(held) = held.and_then(|bytes| codec::read_runs(&bytes, key)) {
                runs = joined_runs(runs, held);
            }
        }
        let bytes = codec::entry(&head, &runs);
        if bytes.len() as u64 > self.max_bytes {
            return Ok(Prepared::Refused);
        }
        self.staging.put(&name, &bytes).await?;
        Ok(Prepared::Ready {
            bytes: bytes.len() as u64,
        })
    }

    async fn staged_value_of(
        &self,
        key: Hash,
        held: &Staged,
    ) -> Result<Option<(Header, Vec<Buffer>)>, String> {
        let Some(meta) = self.staging.get(&staged_name(key, META)).await? else {
            return Ok(None);
        };
        let Some((head, _)) = codec::read_head(&meta, key) else {
            return Ok(None);
        };
        let mut runs = Vec::new();
        for (chunk, _) in &held.chunks {
            let bytes = self.staging.get(chunk).await?;
            match bytes.as_deref().and_then(codec::read_chunk) {
                Some(samples) => joined(&mut runs, vec![Arc::new(samples)]),
                None => return Ok(None),
            }
        }
        let runs = runs.into_iter().map(Arc::unwrap_or_clone).collect();
        Ok(Some((head, runs)))
    }
}

fn joined_runs(staged: Vec<Buffer>, held: Vec<Buffer>) -> Vec<Buffer> {
    let mut runs: Vec<Arc<Buffer>> = held.into_iter().map(Arc::new).collect();
    joined(&mut runs, staged.into_iter().map(Arc::new).collect());
    runs.into_iter().map(Arc::unwrap_or_clone).collect()
}

fn staged_name(key: Hash, part: &str) -> String {
    format!("{}.{part}", name_of(key))
}
