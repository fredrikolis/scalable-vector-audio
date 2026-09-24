// Concern: what one render asked its store, and what each lookup came to | Non-concern: deciding what to store (the store's own worth_storing) | IO: (loads, stores) -> CacheStats

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use sva_formula::Hash;
use sva_samples::{FilterTrace, Label};

use super::{Cache, Entry, Expected, Payload, PayloadKind, Tier};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Hit(Tier),
    ComputedStored,
    ComputedNotStored,
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

    fn count(&self, of: impl Fn(Outcome) -> bool) -> usize {
        self.lookups.iter().filter(|l| of(l.outcome)).count()
    }
}

/// Wraps the store a render was handed, so the render itself never says what it looked up.
pub(crate) struct Recording<'a> {
    inner: &'a dyn Cache,
    lookups: Mutex<Vec<Lookup>>,
}

impl<'a> Recording<'a> {
    pub(crate) fn over(inner: &'a dyn Cache) -> Recording<'a> {
        Recording {
            inner,
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

    fn held(&self) -> MutexGuard<'_, Vec<Lookup>> {
        self.lookups.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Cache for Recording<'_> {
    fn load(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        let found = self.inner.load(key, node, expected);
        self.held().push(Lookup {
            node: node.to_string(),
            key,
            kind: expected.kind(),
            outcome: found
                .as_ref()
                .map_or(Outcome::ComputedNotStored, |e| Outcome::Hit(e.tier)),
        });
        found
    }

    fn store(&self, key: Hash, payload: &Payload, traces: &[FilterTrace], label: Option<&Label>) {
        if let Some(missed) = self
            .held()
            .iter_mut()
            .rev()
            .find(|l| l.key == key && l.outcome == Outcome::ComputedNotStored)
        {
            missed.outcome = Outcome::ComputedStored;
        }
        self.inner.store(key, payload, traces, label);
    }

    fn holds(&self, key: Hash) -> bool {
        self.inner.holds(key)
    }

    fn worth_storing(&self, cost: Duration, bytes: usize, kind: PayloadKind) -> bool {
        self.inner.worth_storing(cost, bytes, kind)
    }

    fn sweep(&self) {
        self.inner.sweep();
    }

    fn held_bytes(&self) -> u64 {
        self.inner.held_bytes()
    }

    fn evicted_bytes(&self) -> u64 {
        self.inner.evicted_bytes()
    }

    fn faults(&self) -> u64 {
        self.inner.faults()
    }

    fn max_bytes(&self) -> u64 {
        self.inner.max_bytes()
    }

    fn dir(&self) -> Option<&Path> {
        self.inner.dir()
    }
}
