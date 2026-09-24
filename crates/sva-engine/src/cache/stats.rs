// Concern: what one render asked its store, and what each lookup came to | Non-concern: deciding what to store (the store's own worth_storing) | IO: (loads, stores) -> CacheStats

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use sva_formula::Hash;
use sva_samples::{FilterTrace, Label};

use super::{Cache, Entry, Expected, Payload, PayloadKind, Put, Slots, Tier};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Hit(Tier),
    ComputedStored,
    ComputedNotStored,
    /// A volatile node's value, kept in a slot that held nothing.
    ComputedSlotted,
    /// A volatile node's value, kept in place of the slot's last one.
    ComputedReplaced,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lookup {
    pub node: String,
    pub key: Hash,
    pub kind: PayloadKind,
    pub outcome: Outcome,
}

/// Every lookup in the order the render made it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CacheStats {
    pub lookups: Vec<Lookup>,
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
        self.count(|o| matches!(o, Outcome::Hit(_)))
    }

    pub fn hits_in(&self, tier: Tier) -> usize {
        self.count(|o| o == Outcome::Hit(tier))
    }

    pub fn computed(&self) -> usize {
        self.count(|o| !matches!(o, Outcome::Hit(_)))
    }

    pub fn stored(&self) -> usize {
        self.count(|o| o == Outcome::ComputedStored)
    }

    pub fn slotted(&self) -> usize {
        self.count(|o| o == Outcome::ComputedSlotted)
    }

    pub fn replaced(&self) -> usize {
        self.count(|o| o == Outcome::ComputedReplaced)
    }

    fn count(&self, of: impl Fn(Outcome) -> bool) -> usize {
        self.lookups.iter().filter(|l| of(l.outcome)).count()
    }
}

/// Wraps the stores a render was handed, so the render itself never says what it looked up.
pub(crate) struct Recording<'a> {
    inner: &'a dyn Cache,
    slots: Option<&'a Slots>,
    lookups: Mutex<Vec<Lookup>>,
}

impl<'a> Recording<'a> {
    pub(crate) fn over(inner: &'a dyn Cache, slots: Option<&'a Slots>) -> Recording<'a> {
        Recording {
            inner,
            slots,
            lookups: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn finish(self) -> CacheStats {
        CacheStats {
            lookups: self
                .lookups
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
        }
    }

    /// One node's view of the stores: a volatile node reads through its slot and writes
    /// nowhere else, every other node reads and writes the stores themselves.
    pub(crate) fn at(&'a self, slot: Option<Hash>) -> Lens<'a> {
        Lens {
            recording: self,
            slot,
        }
    }

    fn held(&self) -> MutexGuard<'_, Vec<Lookup>> {
        self.lookups.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, key: Hash, node: &str, expected: Expected, found: Option<&Entry>) {
        self.held().push(Lookup {
            node: node.to_string(),
            key,
            kind: expected.kind(),
            outcome: found.map_or(Outcome::ComputedNotStored, |e| Outcome::Hit(e.tier)),
        });
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
}

impl Lens<'_> {
    fn slotted(&self, kind: PayloadKind) -> Option<(&Slots, Hash)> {
        let slot = super::mixed(self.slot?, &[kind as u64, 0x73_6c_6f_74]);
        Some((self.recording.slots?, slot))
    }
}

impl Cache for Lens<'_> {
    fn load(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        let inner = self.recording.inner;
        let found = match self.slot {
            None => inner.load(key, node, expected),
            Some(_) => inner.peek(key, node, expected).or_else(|| {
                let (slots, slot) = self.slotted(expected.kind())?;
                slots.get(slot, key, node, expected)
            }),
        };
        self.recording.record(key, node, expected, found.as_ref());
        found
    }

    fn peek(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        self.recording.inner.peek(key, node, expected)
    }

    fn store(&self, key: Hash, payload: &Payload, traces: &[FilterTrace], label: Option<&Label>) {
        if self.slot.is_none() {
            self.recording.settle(key, Outcome::ComputedStored);
            return self.recording.inner.store(key, payload, traces, label);
        }
        let Some((slots, slot)) = self.slotted(payload.kind()) else {
            return;
        };
        match slots.put(slot, key, payload, traces, label) {
            Put::Slotted => self.recording.settle(key, Outcome::ComputedSlotted),
            Put::Replaced => self.recording.settle(key, Outcome::ComputedReplaced),
            Put::Refused => {}
        }
    }

    fn holds(&self, key: Hash) -> bool {
        self.recording.inner.holds(key)
    }

    /// A slot keeps a volatile node's buffer or frames; its spectral sum is kept nowhere.
    fn worth_storing(&self, cost: Duration, bytes: usize, kind: PayloadKind) -> bool {
        match self.slot {
            None => self.recording.inner.worth_storing(cost, bytes, kind),
            Some(_) => kind != PayloadKind::Symbolic && self.recording.slots.is_some(),
        }
    }

    fn sweep(&self) {
        if self.slot.is_none() {
            self.recording.inner.sweep();
        }
    }

    fn held_bytes(&self) -> u64 {
        self.recording.inner.held_bytes()
    }

    fn evicted_bytes(&self) -> u64 {
        self.recording.inner.evicted_bytes()
    }

    fn faults(&self) -> u64 {
        self.recording.inner.faults()
    }

    fn max_bytes(&self) -> u64 {
        self.recording.inner.max_bytes()
    }

    fn dir(&self) -> Option<&Path> {
        self.recording.inner.dir()
    }
}
