// Concern: fronts a persistent store with this process's heap, promoting what the back answers | Non-concern: either tier's medium or budget | IO: (Hash) -> a payload + traces, and its tier

use std::path::Path;
use std::time::Duration;

use sva_formula::Hash;
use sva_samples::{FilterTrace, Label};

use super::{Cache, Entry, Expected, Medium, MemoryCache, Pack, Payload, PayloadKind};

/// The back is a pack and nothing else: `store` carries no cost, and a pack admits by kind alone,
/// so no back gated on cost can sit here and silently keep nothing.
pub struct Tiered<M: Medium> {
    pub front: MemoryCache,
    pub back: Pack<M>,
}

impl<M: Medium> Tiered<M> {
    pub fn new(front: MemoryCache, back: Pack<M>) -> Tiered<M> {
        Tiered { front, back }
    }
}

impl<M: Medium> Cache for Tiered<M> {
    fn load(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        if let Some(entry) = self.front.load(key, node, expected) {
            return Some(entry);
        }
        let entry = self.back.load(key, node, expected)?;
        self.front
            .store(key, &entry.payload, &entry.traces, entry.label.as_ref());
        Some(entry)
    }

    fn store(&self, key: Hash, payload: &Payload, traces: &[FilterTrace], label: Option<&Label>) {
        self.front.store(key, payload, traces, label);
        if self
            .back
            .worth_storing(Duration::ZERO, payload.bytes(), payload.kind())
        {
            self.back.store(key, payload, traces, label);
        }
    }

    fn holds(&self, key: Hash) -> bool {
        self.front.holds(key) || self.back.holds(key)
    }

    fn worth_storing(&self, cost: Duration, bytes: usize, kind: PayloadKind) -> bool {
        self.front.worth_storing(cost, bytes, kind) || self.back.worth_storing(cost, bytes, kind)
    }

    fn sweep(&self) {
        self.front.sweep();
        self.back.sweep();
    }

    /// Both tiers' own bytes, summed: a value in both is held twice.
    fn held_bytes(&self) -> u64 {
        self.front.held_bytes() + self.back.held_bytes()
    }

    fn evicted_bytes(&self) -> u64 {
        self.front.evicted_bytes() + self.back.evicted_bytes()
    }

    fn faults(&self) -> u64 {
        self.front.faults() + self.back.faults()
    }

    fn max_bytes(&self) -> u64 {
        self.front.max_bytes().saturating_add(self.back.max_bytes())
    }

    fn dir(&self) -> Option<&Path> {
        self.back.dir()
    }
}
