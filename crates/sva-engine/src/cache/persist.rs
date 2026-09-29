// Concern: the persistent tier under the in-memory store: version, budget, eviction, the one write | Non-concern: the bytes' medium (a Backend) | IO: (Backend, keys) -> hits; persist() -> writes

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use sva_formula::Hash;

use super::{Cache, codec};

pub const DEFAULT_STORE_BYTES: u64 = 2 << 30;

pub const STORE_VERSION: &str = concat!(
    "sva-engine ",
    env!("CARGO_PKG_VERSION"),
    " build ",
    env!("SVA_ENGINE_BUILD"),
    " format 1"
);

pub const VERSION_NAME: &str = "version";

const RECENCY_NAME: &str = "recency";

/// Named byte storage. A `put` is whole or absent: no reader ever sees half of one.
pub trait Backend {
    fn get(&self, name: &str) -> impl Future<Output = Result<Option<Vec<u8>>, String>>;
    fn put(&self, name: &str, bytes: &[u8]) -> impl Future<Output = Result<(), String>>;
    fn delete(&self, name: &str) -> impl Future<Output = Result<(), String>>;
    /// Every name held, with its size in bytes.
    fn list(&self) -> impl Future<Output = Result<Vec<(String, u64)>, String>>;
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

/// The in-memory store over a persistent one. A render reads through it; only `persist`
/// writes, whole entries least recently used going first past the budget.
pub struct Store<B> {
    backend: B,
    cache: Cache,
    max_bytes: u64,
    index: Mutex<Index>,
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

impl<B: Backend> Store<B> {
    /// A store another version wrote is emptied first.
    pub async fn open(backend: B, cache: Cache, max_bytes: u64) -> Result<Store<B>, String> {
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
        Ok(Store {
            backend,
            cache,
            max_bytes,
            index: Mutex::new(index),
        })
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub fn bytes(&self) -> u64 {
        self.indexed().bytes
    }

    pub fn holds(&self, key: Hash) -> bool {
        self.indexed().held.contains_key(&key)
    }

    fn indexed(&self) -> MutexGuard<'_, Index> {
        self.index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Each key the in-memory store lacks: true a hit loaded into it, false a miss. An entry
    /// that does not decode is a miss, and goes.
    pub(crate) async fn warm(&self, keys: &[Hash]) -> HashMap<Hash, bool> {
        let mut out = HashMap::new();
        for key in keys {
            if out.contains_key(key) || self.cache.holds(*key) {
                continue;
            }
            if !self.holds(*key) {
                out.insert(*key, false);
                continue;
            }
            let name = name_of(*key);
            let bytes = match self.backend.get(&name).await {
                Ok(bytes) => bytes,
                Err(_) => {
                    out.insert(*key, false);
                    continue;
                }
            };
            match bytes.as_deref().map(|b| (b.len(), codec::decode(b))) {
                Some((len, Some(entry))) => {
                    self.indexed().touch(*key, len as u64);
                    self.cache.warmed(*key, entry);
                    out.insert(*key, true);
                }
                held => {
                    if held.is_some() {
                        let _ = self.backend.delete(&name).await;
                    }
                    self.indexed().forget(*key);
                    out.insert(*key, false);
                }
            }
        }
        out
    }

    /// Evicts least recently used whole entries after writing, until the budget holds.
    pub async fn persist(&self) -> Result<Persisted, String> {
        let mut done = Persisted::default();
        for key in self.cache.unpersisted() {
            let Some(bytes) = self.cache.encoded(key) else {
                continue;
            };
            if bytes.len() as u64 > self.max_bytes {
                continue;
            }
            self.backend.put(&name_of(key), &bytes).await?;
            self.cache.persisted(key);
            self.indexed().touch(key, bytes.len() as u64);
            done.written += 1;
        }
        let oldest = self.indexed().oldest_first();
        for key in oldest {
            if self.bytes() <= self.max_bytes {
                break;
            }
            self.backend.delete(&name_of(key)).await?;
            self.indexed().forget(key);
            done.evicted += 1;
        }
        let order: String = self
            .indexed()
            .oldest_first()
            .into_iter()
            .map(|key| name_of(key) + "\n")
            .collect();
        self.backend.put(RECENCY_NAME, order.as_bytes()).await?;
        Ok(done)
    }
}
