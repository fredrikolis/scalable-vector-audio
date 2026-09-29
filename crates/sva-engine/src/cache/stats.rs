// Concern: what one render asked the store, how far its output got, what each lookup came to, and the store after it | Non-concern: what the store evicts (store.rs) | IO: (loads, stores) -> CacheStats

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use sva_samples::MachineState;

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
    /// A run found short, carried on and stored again.
    Extended,
    /// Found only up to a switch.
    Prefix,
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
    /// Each output sample a pull reached, and how many lookups had been made by then.
    pub reached: Vec<(i64, usize)>,
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

    pub fn extended(&self) -> usize {
        self.count(|o| o == Outcome::Extended)
    }

    fn count(&self, of: impl Fn(Outcome) -> bool) -> usize {
        self.lookups.iter().filter(|l| of(l.outcome)).count()
    }
}

/// One render's view of the store, so the render itself never says what it looked up.
pub(crate) struct Recording {
    cache: Cache,
    policy: CachePolicy,
    tree: u64,
    evictions: u64,
    lookups: Mutex<Vec<Lookup>>,
    reached: Mutex<Vec<(i64, usize)>>,
}

impl Recording {
    /// `policy` where the render names one, the store's own where it names none.
    pub(crate) fn over(cache: &Cache, policy: Option<CachePolicy>) -> Recording {
        Recording {
            cache: cache.clone(),
            policy: policy.unwrap_or_else(|| cache.policy()),
            tree: cache.begin_tree(),
            evictions: cache.evictions(),
            lookups: Mutex::new(Vec::new()),
            reached: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn reach(&self, at: i64) {
        let made = self.held().len();
        self.reached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((at, made));
    }

    pub(crate) fn finish(self) -> CacheStats {
        self.stats()
    }

    pub(crate) fn stats(&self) -> CacheStats {
        CacheStats {
            lookups: self.held().clone(),
            bytes: self.cache.bytes(),
            max_bytes: self.cache.max_bytes(),
            entries: self.cache.entries(),
            evictions: self.cache.evictions() - self.evictions,
            reached: self
                .reached
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
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

    pub(crate) fn run_span(&self, key: Hash) -> Option<(sva_samples::Extent, Option<Hash>)> {
        self.cache.run_span(key)
    }

    pub(crate) fn mark_every(&self) -> usize {
        self.cache.mark_every()
    }

    fn held(&self) -> MutexGuard<'_, Vec<Lookup>> {
        self.lookups.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn settle(&self, key: Hash, outcome: Outcome) {
        let mut held = self.held();
        let last = held.iter_mut().rev().find(|l| l.key == key);
        match last {
            Some(missed) if missed.outcome == Outcome::ComputedNotStored => {
                missed.outcome = outcome;
            }
            Some(hit) if hit.outcome == Outcome::Hit && hit.kind == PayloadKind::Run => {
                hit.outcome = Outcome::Extended;
            }
            _ => {}
        }
    }
}

pub(crate) struct Lens<'a> {
    recording: &'a Recording,
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

    /// Read unnoted; its node notes one lookup.
    pub(crate) fn peek(&self, key: Hash, expected: Expected) -> Option<Entry> {
        let stamp = self.stamp(expected.kind());
        self.recording.cache.load(key, expected, stamp)
    }

    pub(crate) fn note(&self, node: &str, key: Hash, outcome: Outcome) {
        self.recording.held().push(Lookup {
            node: node.to_string(),
            key,
            kind: PayloadKind::Run,
            outcome,
        });
    }

    pub(crate) fn mark(&self, key: Hash, marks: BTreeMap<i64, MachineState>) {
        if self.stores {
            self.recording.cache.mark(key, marks);
        }
    }

    /// Read after its children, so they are evicted first.
    pub(crate) fn touch(&self, key: Hash) {
        self.recording.cache.touch(key);
    }

    /// A value looked up under `looked` and computed over less than it named.
    pub(crate) fn rekey(&self, looked: Hash, now: Hash) {
        for lookup in self.recording.held().iter_mut() {
            if lookup.key == looked && lookup.outcome == Outcome::ComputedNotStored {
                lookup.key = now;
            }
        }
    }

    pub(crate) fn keeps(&self) -> bool {
        self.stores
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
