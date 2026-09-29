// Concern: the one bounded store, what it evicts and the cap it never passes | Non-concern: what a render asks of it (stats.rs) | IO: (Hash) -> a payload + label

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use sva_formula::Hash;
use sva_samples::Label;

use super::{Entry, Expected, Payload};

pub const DEFAULT_CACHE_BYTES: u64 = 2 << 30;

/// Samples between two states a run keeps, so a reader resumes from one at most this far back.
pub const DEFAULT_MARK_EVERY: usize = 16_384;

/// Which values a render stores; whatever is not stored is computed again when asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CachePolicy {
    #[default]
    All,
    /// The values two or more nodes read, and the render target.
    Forks,
    Target,
    None,
}

impl CachePolicy {
    pub const ALL: [CachePolicy; 4] = [
        CachePolicy::All,
        CachePolicy::Forks,
        CachePolicy::Target,
        CachePolicy::None,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CachePolicy::All => "all",
            CachePolicy::Forks => "forks",
            CachePolicy::Target => "target",
            CachePolicy::None => "none",
        }
    }

    pub fn named(name: &str) -> Option<CachePolicy> {
        CachePolicy::ALL.into_iter().find(|p| p.name() == name)
    }

    pub(crate) fn stores(self, fork: bool, target: bool) -> bool {
        match self {
            CachePolicy::All => true,
            CachePolicy::Forks => fork || target,
            CachePolicy::Target => target,
            CachePolicy::None => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrunePolicy {
    /// Every entry the newest render neither stored nor read.
    #[default]
    Oldest,
    /// Every entry whose node fewer than two nodes read.
    Forks,
}

impl PrunePolicy {
    pub const ALL: [PrunePolicy; 2] = [PrunePolicy::Oldest, PrunePolicy::Forks];

    pub fn name(self) -> &'static str {
        match self {
            PrunePolicy::Oldest => "oldest",
            PrunePolicy::Forks => "forks",
        }
    }

    pub fn named(name: &str) -> Option<PrunePolicy> {
        PrunePolicy::ALL.into_iter().find(|p| p.name() == name)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Stamp {
    pub tree: u64,
    pub fork: bool,
    /// A volatile node's value replaces the last one stored under the same slot.
    pub slot: Option<Hash>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kept {
    Held,
    Replaced,
    Refused,
}

struct Held {
    payload: Payload,
    label: Option<Label>,
    read: u64,
    tree: u64,
    fork: bool,
    slot: Option<Hash>,
}

impl Held {
    fn bytes(&self) -> u64 {
        self.payload.bytes() as u64
    }
}

#[derive(Default)]
struct State {
    entries: HashMap<Hash, Held>,
    slots: HashMap<Hash, Hash>,
    bytes: u64,
    max_bytes: u64,
    policy: CachePolicy,
    prune: PrunePolicy,
    clock: u64,
    tree: u64,
    evictions: u64,
    mark_every: usize,
}

impl State {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn remove(&mut self, key: Hash) -> bool {
        let Some(gone) = self.entries.remove(&key) else {
            return false;
        };
        self.bytes -= gone.bytes();
        if let Some(slot) = gone.slot
            && self.slots.get(&slot) == Some(&key)
        {
            self.slots.remove(&slot);
        }
        true
    }

    fn evict(&mut self, key: Hash) {
        if self.remove(key) {
            self.evictions += 1;
        }
    }

    /// Every entry `policy` names, oldest-read first, until `bytes` is at most `to`; then whole
    /// trees oldest-first, until the cap holds.
    fn prune(&mut self, policy: PrunePolicy, to: u64) {
        let newest = self.tree;
        let mut named: Vec<(u64, Hash)> = self
            .entries
            .iter()
            .filter(|(_, held)| match policy {
                PrunePolicy::Oldest => held.tree != newest,
                PrunePolicy::Forks => !held.fork,
            })
            .map(|(key, held)| (held.read, *key))
            .collect();
        named.sort_unstable();
        for (_, key) in named {
            if self.bytes <= to {
                break;
            }
            self.evict(key);
        }
        let mut trees: Vec<u64> = self.entries.values().map(|held| held.tree).collect();
        trees.sort_unstable();
        trees.dedup();
        for tree in trees {
            if self.bytes <= self.max_bytes {
                break;
            }
            let whole: Vec<Hash> = self
                .entries
                .iter()
                .filter(|(_, held)| held.tree == tree)
                .map(|(key, _)| *key)
                .collect();
            for key in whole {
                self.evict(key);
            }
        }
    }

    fn bounded(&mut self) {
        if self.bytes > self.max_bytes {
            self.prune(self.prune, self.max_bytes);
        }
    }
}

/// Every value a render computed and chose to keep, under its content hash. A hit is the value
/// the cold run wrote, so what is kept or evicted decides only what is computed again. A clone
/// is a handle on the same store, which a stream holds for as long as it plays.
#[derive(Clone)]
pub struct Cache {
    state: Arc<Mutex<State>>,
}

impl Default for Cache {
    fn default() -> Cache {
        Cache::new()
    }
}

impl Cache {
    pub fn new() -> Cache {
        Cache::holding(DEFAULT_CACHE_BYTES)
    }

    pub fn holding(max_bytes: u64) -> Cache {
        Cache {
            state: Arc::new(Mutex::new(State {
                max_bytes,
                mark_every: DEFAULT_MARK_EVERY,
                ..State::default()
            })),
        }
    }

    fn locked(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| {
            let mut state = poisoned.into_inner();
            state.entries.clear();
            state.slots.clear();
            state.bytes = 0;
            self.state.clear_poison();
            state
        })
    }

    pub fn max_bytes(&self) -> u64 {
        self.locked().max_bytes
    }

    pub fn set_max_bytes(&self, max_bytes: u64) {
        let mut state = self.locked();
        state.max_bytes = max_bytes;
        state.bounded();
    }

    /// What a render stores where it names no policy of its own.
    pub fn policy(&self) -> CachePolicy {
        self.locked().policy
    }

    pub fn set_policy(&self, policy: CachePolicy) {
        self.locked().policy = policy;
    }

    pub fn prune_policy(&self) -> PrunePolicy {
        self.locked().prune
    }

    pub fn set_prune_policy(&self, policy: PrunePolicy) {
        self.locked().prune = policy;
    }

    /// Evicts every entry `policy` names, and more where the cap still needs it.
    pub fn prune(&self, policy: PrunePolicy) {
        self.locked().prune(policy, 0);
    }

    pub fn clear(&self) {
        let mut state = self.locked();
        state.entries.clear();
        state.slots.clear();
        state.bytes = 0;
    }

    pub fn bytes(&self) -> u64 {
        self.locked().bytes
    }

    pub fn entries(&self) -> usize {
        self.locked().entries.len()
    }

    pub fn evictions(&self) -> u64 {
        self.locked().evictions
    }

    pub fn holds(&self, key: Hash) -> bool {
        self.locked().entries.contains_key(&key)
    }

    /// How many samples apart a run keeps the states it passes.
    pub fn mark_every(&self) -> usize {
        self.locked().mark_every
    }

    pub fn set_mark_every(&self, samples: usize) {
        self.locked().mark_every = samples.max(1);
    }

    pub(crate) fn begin_tree(&self) -> u64 {
        let mut state = self.locked();
        state.tree += 1;
        state.tree
    }

    pub(crate) fn load(&self, key: Hash, expected: Expected, stamp: Stamp) -> Option<Entry> {
        let mut state = self.locked();
        let tick = state.tick();
        let held = state.entries.get_mut(&key)?;
        if !held.payload.answers(expected) {
            state.remove(key);
            return None;
        }
        held.read = tick;
        held.tree = stamp.tree;
        held.fork = stamp.fork;
        Some(Entry {
            payload: held.payload.clone(),
            label: held.label.clone(),
        })
    }

    /// A value's segments join those held under `key`, and a run continuing the one held
    /// there extends it, each in place; anything else replaces what `key` held.
    pub(crate) fn merge(
        &self,
        key: Hash,
        payload: Payload,
        label: Option<&Label>,
        stamp: Stamp,
    ) -> Kept {
        let mut state = self.locked();
        let tick = state.tick();
        let joined = match (state.entries.get_mut(&key), payload) {
            (Some(held), payload) if held.slot == stamp.slot => {
                let before = held.bytes();
                let payload = match (&mut held.payload, payload) {
                    (Payload::Segments(parts), Payload::Segments(more)) => {
                        joined(parts, more);
                        None
                    }
                    (Payload::Run(run), Payload::Run(more)) if overlaps(run, &more) => {
                        let from = (run.end() - more.samples.start).max(0) as usize;
                        for (held, more) in run.samples.planes.iter_mut().zip(&more.samples.planes)
                        {
                            held.extend_from_slice(&more[from.min(more.len())..]);
                        }
                        run.marks.extend(more.marks);
                        None
                    }
                    (_, payload) => Some(payload),
                };
                match payload {
                    None => {
                        held.read = tick;
                        held.tree = stamp.tree;
                        held.fork = stamp.fork;
                        let after = held.bytes();
                        Ok((before, after))
                    }
                    Some(payload) => Err(payload),
                }
            }
            (_, payload) => Err(payload),
        };
        match joined {
            Ok((before, after)) => {
                state.bytes = state.bytes - before + after;
                state.bounded();
                Kept::Held
            }
            Err(payload) => {
                drop(state);
                self.store(key, &payload, label, stamp)
            }
        }
    }

    pub(crate) fn store(
        &self,
        key: Hash,
        payload: &Payload,
        label: Option<&Label>,
        stamp: Stamp,
    ) -> Kept {
        let mut state = self.locked();
        let bytes = payload.bytes() as u64;
        if bytes > state.max_bytes {
            return Kept::Refused;
        }
        let read = state.tick();
        let replaced = match stamp.slot.and_then(|slot| state.slots.insert(slot, key)) {
            Some(last) if last != key => state.remove(last),
            _ => false,
        };
        let held = Held {
            payload: payload.clone(),
            label: label.cloned(),
            read,
            tree: stamp.tree,
            fork: stamp.fork,
            slot: stamp.slot,
        };
        if let Some(old) = state.entries.insert(key, held) {
            state.bytes -= old.bytes();
        }
        state.bytes += bytes;
        state.bounded();
        match replaced {
            true => Kept::Replaced,
            false => Kept::Held,
        }
    }
}

/// A run that starts inside or at the end of the one held continues it: what it holds past
/// that one's end is laid on, the samples both hold being the same.
fn overlaps(held: &super::Run, more: &super::Run) -> bool {
    let (a, b) = (held.samples.start, held.end());
    a <= more.samples.start && more.samples.start <= b
}

/// `more` laid among `parts`, each touching pair joined into one.
fn joined(parts: &mut Vec<sva_samples::Buffer>, more: Vec<sva_samples::Buffer>) {
    for part in more {
        parts.push(part);
    }
    parts.sort_by_key(|b| b.start);
    let mut out: Vec<sva_samples::Buffer> = Vec::with_capacity(parts.len());
    for part in parts.drain(..) {
        match out.last_mut() {
            Some(last) if last.extent().end >= part.start => {
                let from = (last.extent().end - part.start) as usize;
                for (held, more) in last.planes.iter_mut().zip(&part.planes) {
                    held.extend_from_slice(&more[from.min(more.len())..]);
                }
            }
            _ => out.push(part),
        }
    }
    *parts = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use sva_samples::Buffer;

    /// Only a colliding key reaches this, so no render can: the entry is a miss, and goes.
    #[test]
    fn an_entry_that_does_not_answer_what_was_asked_is_a_miss_and_goes() {
        let cache = Cache::new();
        let key = Hash(7, 11);
        let stamp = Stamp {
            tree: cache.begin_tree(),
            fork: false,
            slot: None,
        };
        let four = Payload::Segments(vec![Buffer::mono(8_000, vec![0.25; 4])]);
        for (rate, width) in [(48_000, 1), (8_000, 2)] {
            cache.store(key, &four, None, stamp);
            let asked = Expected::Segments { rate, width };
            assert!(cache.load(key, asked, stamp).is_none());
            assert!(!cache.holds(key));
            assert_eq!(cache.bytes(), 0);
        }
    }
}
