// Concern: keeps rendered buffers in this process's own heap under their content hash | Non-concern: what a store is for (cache.rs) | IO: (Hash) -> a buffer + traces

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sva_samples::{FilterTrace, Label};

use sva_formula::Hash;

use super::evict;
use super::{Cache, Entry, Expected, Payload, PayloadKind, Tier};

/// What a process can hold, not what a filesystem can.
pub const DEFAULT_MAX_BYTES: u64 = 2 << 30;

struct Held {
    payload: Payload,
    traces: Vec<FilterTrace>,
    label: Option<Label>,
    read: u64,
}

impl Held {
    fn bytes(&self) -> u64 {
        self.payload.bytes() as u64
    }

    fn entry(&self, node: &str) -> Entry {
        Entry {
            payload: self.payload.clone(),
            traces: super::renamed(&self.traces, node),
            label: self.label.clone(),
            tier: Tier::Memory,
        }
    }
}

pub struct MemoryCache {
    entries: Mutex<BTreeMap<Hash, Held>>,
    max_bytes: u64,
    clock: AtomicU64,
    held: AtomicU64,
    evicted: AtomicU64,
}

impl MemoryCache {
    pub fn new() -> MemoryCache {
        MemoryCache::holding(DEFAULT_MAX_BYTES)
    }

    pub fn holding(max_bytes: u64) -> MemoryCache {
        MemoryCache {
            entries: Mutex::new(BTreeMap::new()),
            max_bytes,
            clock: AtomicU64::new(0),
            held: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
        }
    }

    /// A holder that panicked mid-mutation may have left an entry half-written, and every
    /// entry is derived data: the store empties, charged as evicted, and keeps serving.
    fn locked(&self) -> std::sync::MutexGuard<'_, BTreeMap<Hash, Held>> {
        match self.entries.lock() {
            Ok(entries) => entries,
            Err(poisoned) => {
                let mut entries = poisoned.into_inner();
                let dropped: u64 = entries.values().map(Held::bytes).sum();
                entries.clear();
                self.held.store(0, Ordering::Relaxed);
                self.evicted.store(dropped, Ordering::Relaxed);
                self.entries.clear_poison();
                entries
            }
        }
    }
}

impl Default for MemoryCache {
    fn default() -> MemoryCache {
        MemoryCache::new()
    }
}

impl Cache for MemoryCache {
    fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    fn held_bytes(&self) -> u64 {
        self.held.load(Ordering::Relaxed)
    }

    fn evicted_bytes(&self) -> u64 {
        self.evicted.load(Ordering::Relaxed)
    }

    fn holds(&self, key: Hash) -> bool {
        self.locked().contains_key(&key)
    }

    fn load(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.locked();
        let held = entries.get_mut(&key)?;
        if !held.payload.answers(expected) {
            entries.remove(&key);
            return None;
        }
        held.read = tick;
        Some(held.entry(node))
    }

    fn peek(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        let entries = self.locked();
        let held = entries.get(&key)?;
        held.payload.answers(expected).then(|| held.entry(node))
    }

    /// A hit is a memcpy, so anything computed is kept until the budget says otherwise.
    fn worth_storing(&self, _cost: Duration, _bytes: usize, _kind: PayloadKind) -> bool {
        true
    }

    fn store(&self, key: Hash, payload: &Payload, traces: &[FilterTrace], label: Option<&Label>) {
        let read = self.clock.fetch_add(1, Ordering::Relaxed);
        self.locked().insert(
            key,
            Held {
                payload: payload.clone(),
                traces: traces.to_vec(),
                label: label.cloned(),
                read,
            },
        );
    }

    /// The read time is the tick `load` stamped the entry with.
    fn sweep(&self) {
        let mut entries = self.locked();
        let order: Vec<(u64, u64, Hash)> = entries
            .iter()
            .map(|(k, h)| (h.read, h.bytes(), *k))
            .collect();
        let swept = evict::to_cap(order, self.max_bytes, |key| entries.remove(key).is_some());
        self.held.store(swept.held, Ordering::Relaxed);
        self.evicted.store(swept.evicted, Ordering::Relaxed);
    }
}
