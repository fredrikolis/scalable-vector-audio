// Concern: what one render asked of its values and the store, how far its output got, and what each lookup came to | Non-concern: what the store evicts (store.rs) | IO: (loads, stores) -> CacheStats

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
    /// What a persistent store answered.
    pub store: Option<bool>,
}

/// Every lookup in order, and the store as the render left it.
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
    pub typed: Vec<String>,
    pub planned: Vec<String>,
}

impl CacheStats {
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

pub(crate) struct Recording {
    cache: Option<Cache>,
    policy: CachePolicy,
    tree: u64,
    evictions: u64,
    lookups: Vec<Lookup>,
    reached: Vec<(i64, usize)>,
}

impl Recording {
    /// `policy` where the render names one, else the store's own.
    pub(crate) fn over(cache: Option<&Cache>, policy: Option<CachePolicy>) -> Recording {
        Recording {
            cache: cache.cloned(),
            policy: policy.unwrap_or_else(|| cache.map_or(CachePolicy::None, Cache::policy)),
            tree: cache.map_or(0, Cache::begin_tree),
            evictions: cache.map_or(0, Cache::evictions),
            lookups: Vec::new(),
            reached: Vec::new(),
        }
    }

    pub(crate) fn reach(&mut self, at: i64) {
        self.reached.push((at, self.lookups.len()));
    }

    pub(crate) fn stats(&self) -> CacheStats {
        let cache = self.cache.as_ref();
        CacheStats {
            lookups: self.lookups.clone(),
            bytes: cache.map_or(0, Cache::bytes),
            max_bytes: cache.map_or(0, Cache::max_bytes),
            entries: cache.map_or(0, Cache::entries),
            evictions: cache.map_or(0, |c| c.evictions() - self.evictions),
            reached: self.reached.clone(),
            typed: Vec::new(),
            planned: Vec::new(),
        }
    }

    pub(crate) fn stores(&self, fork: bool, target: bool) -> bool {
        self.cache.is_some() && self.policy.stores(fork, target)
    }

    pub(crate) fn mark_every(&self) -> usize {
        self.cache.as_ref().map_or(usize::MAX, Cache::mark_every)
    }

    /// `slot` where a volatile parameter reaches it, `fork` where two values read it.
    pub(crate) fn stamp(&self, slot: Option<Hash>, fork: bool, kind: PayloadKind) -> Stamp {
        Stamp {
            tree: self.tree,
            fork,
            slot: slot.map(|slot| super::mixed(slot, &[kind as u64, 0x73_6c_6f_74])),
        }
    }

    pub(crate) fn note(
        &mut self,
        node: &str,
        key: Hash,
        kind: PayloadKind,
        outcome: Outcome,
    ) -> usize {
        self.lookups.push(Lookup {
            node: node.to_string(),
            key,
            kind,
            outcome,
            store: None,
        });
        self.lookups.len() - 1
    }

    /// Read unnoted; its value notes one lookup.
    pub(crate) fn load(&self, key: Hash, expected: Expected, stamp: Stamp) -> Option<Entry> {
        self.cache.as_ref()?.load(key, expected, stamp)
    }

    /// What a value computed, merged into what the store holds of it.
    pub(crate) fn store(
        &mut self,
        (key, noted): (Hash, Option<usize>),
        payload: Payload,
        label: Option<&Label>,
        stamp: Stamp,
    ) {
        let Some(cache) = &self.cache else {
            return;
        };
        let outcome = match cache.merge(key, payload, label, stamp) {
            Kept::Held => Outcome::ComputedStored,
            Kept::Replaced => Outcome::ComputedReplaced,
            Kept::Refused => return,
        };
        let Some(lookup) = noted.and_then(|at| self.lookups.get_mut(at)) else {
            return;
        };
        match lookup.outcome {
            Outcome::ComputedNotStored => lookup.outcome = outcome,
            Outcome::Hit if lookup.kind == PayloadKind::Run => lookup.outcome = Outcome::Extended,
            _ => {}
        }
    }
}
