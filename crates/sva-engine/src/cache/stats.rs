// Concern: what one render asked the store, what each lookup came to, and the store after it | Non-concern: what the store evicts (store.rs) | IO: (loads, stores) -> CacheStats

use std::sync::{Mutex, MutexGuard, PoisonError};

use sva_formula::Hash;
use sva_samples::Label;

use super::store::{Kept, Stamp};
use super::{Cache, CachePolicy, Entry, Expected, Payload, PayloadKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Hit,
    ComputedStored,
    ComputedNotStored,
    /// A volatile node's value, stored in place of its last one.
    ComputedReplaced,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lookup {
    pub node: String,
    pub key: Hash,
    pub kind: PayloadKind,
    pub outcome: Outcome,
}

/// Every lookup in the order the render made it, and the store as the render left it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CacheStats {
    pub lookups: Vec<Lookup>,
    pub bytes: u64,
    pub max_bytes: u64,
    pub entries: usize,
    /// Entries this render's stores evicted.
    pub evictions: u64,
}

impl CacheStats {
    /// Distinct nodes looked up, however many kinds each asked for.
    pub fn nodes(&self) -> usize {
        let mut names: Vec<&str> = self.lookups.iter().map(|l| l.node.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names.len()
    }

    pub fn hits(&self) -> usize {
        self.count(|o| o == Outcome::Hit)
    }

    pub fn computed(&self) -> usize {
        self.count(|o| o != Outcome::Hit)
    }

    pub fn stored(&self) -> usize {
        self.count(|o| o == Outcome::ComputedStored)
    }

    pub fn replaced(&self) -> usize {
        self.count(|o| o == Outcome::ComputedReplaced)
    }

    fn count(&self, of: impl Fn(Outcome) -> bool) -> usize {
        self.lookups.iter().filter(|l| of(l.outcome)).count()
    }
}

/// One render's view of the store, so the render itself never says what it looked up.
pub(crate) struct Recording<'a> {
    cache: &'a Cache,
    policy: CachePolicy,
    tree: u64,
    evictions: u64,
    lookups: Mutex<Vec<Lookup>>,
}

impl<'a> Recording<'a> {
    /// `policy` where the render names one, the store's own where it names none.
    pub(crate) fn over(cache: &'a Cache, policy: Option<CachePolicy>) -> Recording<'a> {
        Recording {
            cache,
            policy: policy.unwrap_or_else(|| cache.policy()),
            tree: cache.begin_tree(),
            evictions: cache.evictions(),
            lookups: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn finish(self) -> CacheStats {
        CacheStats {
            lookups: self
                .lookups
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
            bytes: self.cache.bytes(),
            max_bytes: self.cache.max_bytes(),
            entries: self.cache.entries(),
            evictions: self.cache.evictions() - self.evictions,
        }
    }

    /// One node's view: `slot` where a volatile parameter reaches it, `fork` where two nodes
    /// read it, `target` where the render is of it.
    pub(crate) fn at(&self, slot: Option<Hash>, fork: bool, target: bool) -> Lens<'_> {
        Lens {
            recording: self,
            slot,
            fork,
            stores: self.stores(fork, target),
        }
    }

    pub(crate) fn stores(&self, fork: bool, target: bool) -> bool {
        self.policy.stores(fork, target)
    }

    pub(crate) fn holds(&self, key: Hash) -> bool {
        self.cache.holds(key)
    }

    fn held(&self) -> MutexGuard<'_, Vec<Lookup>> {
        self.lookups.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn settle(&self, key: Hash, outcome: Outcome) {
        if let Some(missed) = self
            .held()
            .iter_mut()
            .rev()
            .find(|l| l.key == key && l.outcome == Outcome::ComputedNotStored)
        {
            missed.outcome = outcome;
        }
    }
}

pub(crate) struct Lens<'a> {
    recording: &'a Recording<'a>,
    slot: Option<Hash>,
    fork: bool,
    stores: bool,
}

impl Lens<'_> {
    fn stamp(&self, kind: PayloadKind) -> Stamp {
        Stamp {
            tree: self.recording.tree,
            fork: self.fork,
            slot: self
                .slot
                .map(|slot| super::mixed(slot, &[kind as u64, 0x73_6c_6f_74])),
        }
    }

    pub(crate) fn load(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        let found = self
            .recording
            .cache
            .load(key, expected, self.stamp(expected.kind()));
        self.recording.held().push(Lookup {
            node: node.to_string(),
            key,
            kind: expected.kind(),
            outcome: match found {
                Some(_) => Outcome::Hit,
                None => Outcome::ComputedNotStored,
            },
        });
        found
    }

    pub(crate) fn store(&self, key: Hash, payload: &Payload, label: Option<&Label>) {
        if !self.stores {
            return;
        }
        let stamp = self.stamp(payload.kind());
        match self.recording.cache.store(key, payload, label, stamp) {
            Kept::Held => self.recording.settle(key, Outcome::ComputedStored),
            Kept::Replaced => self.recording.settle(key, Outcome::ComputedReplaced),
            Kept::Refused => {}
        }
    }
}
