// Concern: what one render asked of its values and memory, how far its output got, and what each lookup came to | Non-concern: what memory evicts (memory.rs) | IO: (loads, stores) -> CacheStats

use std::collections::HashMap;

use sva_formula::Hash;

use super::PayloadKind;
use super::memory::{Counters, Kept, Memory};
use crate::recent::Recent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Hit,
    ComputedStored,
    ComputedNotStored,
    /// A volatile node's value, stored in place of its last one.
    ComputedReplaced,
    /// Found short, the rest computed and stored.
    Extended,
    /// Found only up to a switch.
    Prefix,
    Reused,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lookup {
    pub node: String,
    pub key: Hash,
    pub kind: PayloadKind,
    pub outcome: Outcome,
}

/// Every lookup in order, or a stream's latest, and memory as the render left it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CacheStats {
    pub lookups: Vec<Lookup>,
    /// Lookups made before `lookups`.
    pub shed: usize,
    pub bytes: u64,
    pub max_bytes: u64,
    pub entries: usize,
    pub evictions: u64,
    pub tier: Counters,
    /// Each output sample a pull reached, and how many lookups had been made by then.
    pub reached: Vec<(i64, usize)>,
    pub typed: Vec<String>,
    pub planned: Vec<String>,
    /// Why memory stopped writing to the disk meanwhile.
    pub unstaged: Option<String>,
    pub unslotted: Option<String>,
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
        self.count(|o| !matches!(o, Outcome::Hit | Outcome::Reused))
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

    pub fn reused(&self) -> usize {
        self.count(|o| o == Outcome::Reused)
    }

    fn count(&self, of: impl Fn(Outcome) -> bool) -> usize {
        self.lookups.iter().filter(|l| of(l.outcome)).count()
    }
}

/// What the memo told one render or stream: one lookup per key a change asks.
pub(crate) struct Recording {
    since: Counters,
    failures: u64,
    lookups: Recent<Lookup>,
    reached: Option<Vec<(i64, usize)>>,
    /// Each key's latest lookup still held, and the change that made it.
    looked: HashMap<Hash, (u64, usize)>,
    change: u64,
}

impl Recording {
    pub(crate) fn over(memory: &Memory) -> Recording {
        Recording {
            since: memory.counters(),
            failures: memory.failures().0,
            lookups: Recent::keeping(usize::MAX),
            reached: Some(Vec::new()),
            looked: HashMap::new(),
            change: 0,
        }
    }

    pub(crate) fn latest(self, kept: usize) -> Recording {
        Recording {
            lookups: Recent::keeping(kept),
            reached: None,
            ..self
        }
    }

    /// A stream's change: each key it asks is a lookup anew.
    pub(crate) fn begin(&mut self) {
        self.change += 1;
        let shed = self.lookups.shed();
        self.looked.retain(|_, (_, at)| *at >= shed);
    }

    pub(crate) fn reach(&mut self, at: i64) {
        let made = self.lookups.made();
        if let Some(reached) = &mut self.reached {
            reached.push((at, made));
        }
    }

    pub(crate) fn stats(&self, memory: &Memory) -> CacheStats {
        let tier = self.since(memory);
        let (failures, why) = memory.failures();
        let failed = failures > self.failures;
        CacheStats {
            lookups: self.lookups.iter().cloned().collect(),
            shed: self.lookups.shed(),
            bytes: memory.bytes(),
            max_bytes: memory.max_bytes(),
            entries: memory.entries(),
            evictions: tier.evictions(),
            tier,
            reached: self.reached.clone().unwrap_or_default(),
            typed: Vec::new(),
            planned: Vec::new(),
            unstaged: why.filter(|_| failed || memory.blocked()),
            unslotted: None,
        }
    }

    pub(crate) fn since(&self, memory: &Memory) -> Counters {
        memory.counters().since(self.since)
    }

    /// Asked again in one change, a key's lookup is settled by what any answer found.
    pub(super) fn answered(&mut self, key: Hash, node: &str, kind: PayloadKind, outcome: Outcome) {
        let now = self.change;
        let held = self.looked(key).filter(|(change, _)| *change == now);
        if let Some(lookup) = held.and_then(|(_, at)| self.lookups.get_mut(at)) {
            let found = |o: Outcome| match o {
                Outcome::Hit => 2,
                Outcome::Prefix => 1,
                _ => 0,
            };
            lookup.kind = kind;
            if found(outcome) > found(lookup.outcome) {
                lookup.outcome = outcome;
            }
            return;
        }
        let lookup = Lookup {
            node: node.to_string(),
            key,
            kind,
            outcome,
        };
        let at = self.lookups.push(lookup);
        self.looked.insert(key, (self.change, at));
    }

    pub(super) fn kept(&mut self, key: Hash, kept: Kept) {
        let Some(lookup) = self
            .looked(key)
            .and_then(|(_, at)| self.lookups.get_mut(at))
        else {
            return;
        };
        lookup.outcome = match (lookup.outcome, kept) {
            (Outcome::ComputedNotStored, Kept::Held) => Outcome::ComputedStored,
            (Outcome::ComputedNotStored, Kept::Replaced) => Outcome::ComputedReplaced,
            (Outcome::Hit, Kept::Held | Kept::Replaced) => Outcome::Extended,
            (outcome, _) => outcome,
        };
    }

    pub(crate) fn reused(&mut self, key: Hash, node: &str, kind: PayloadKind) {
        if self.looked(key).is_none() {
            return;
        }
        self.lookups.push(Lookup {
            node: node.to_string(),
            key,
            kind,
            outcome: Outcome::Reused,
        });
    }

    fn looked(&self, key: Hash) -> Option<(u64, usize)> {
        self.looked.get(&key).copied()
    }
}
