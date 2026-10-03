// Concern: what one render asked of its values and memory, how far its output got, and what each lookup came to | Non-concern: what memory evicts (memory.rs) | IO: (loads, stores) -> CacheStats

use sva_formula::Hash;
use sva_samples::Label;

use super::memory::{Counters, Kept, Memory, Stamp};
use super::{Entry, Expected, Payload, PayloadKind};
use crate::recent::Recent;

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
    /// What memory answered a node with.
    pub store: Option<bool>,
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
        self.lookups
            .iter()
            .filter(|l| l.outcome == Outcome::Hit)
            .count()
    }

    /// Every value computed: a node memory missed computes nothing until its values do.
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
        let values = self.lookups.iter().filter(|l| l.store.is_none());
        values.filter(|l| of(l.outcome)).count()
    }
}

pub(crate) struct Recording {
    memory: Memory,
    since: Counters,
    failures: u64,
    lookups: Recent<Lookup>,
    reached: Option<Vec<(i64, usize)>>,
}

impl Recording {
    pub(crate) fn over(memory: &Memory) -> Recording {
        Recording {
            memory: memory.clone(),
            since: memory.counters(),
            failures: memory.failures().0,
            lookups: Recent::keeping(usize::MAX),
            reached: Some(Vec::new()),
        }
    }

    pub(crate) fn latest(self, kept: usize) -> Recording {
        Recording {
            lookups: Recent::keeping(kept),
            reached: None,
            ..self
        }
    }

    pub(crate) fn found(&mut self, lookups: Vec<Lookup>) {
        for lookup in lookups {
            self.lookups.push(lookup);
        }
    }

    pub(crate) fn reach(&mut self, at: i64) {
        let made = self.lookups.made();
        if let Some(reached) = &mut self.reached {
            reached.push((at, made));
        }
    }

    pub(crate) fn stats(&self) -> CacheStats {
        let memory = &self.memory;
        let tier = memory.counters().since(self.since);
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

    pub(crate) fn since(&self) -> Counters {
        self.memory.counters().since(self.since)
    }

    pub(crate) fn keeps(&self) -> bool {
        self.memory.keeps()
    }

    pub(crate) fn mark_every(&self) -> usize {
        self.memory.mark_every()
    }

    /// `slot` where a volatile parameter reaches it.
    pub(crate) fn stamp(&self, slot: Option<Hash>, kind: PayloadKind) -> Stamp {
        Stamp {
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
        })
    }

    /// Read unnoted; its value notes one lookup.
    pub(crate) fn load(&self, key: Hash, expected: Expected) -> Option<Entry> {
        self.memory.load(key, expected)
    }

    /// What a value computed, merged into what memory holds of it.
    pub(crate) fn store(
        &mut self,
        (key, noted): (Hash, Option<usize>),
        payload: Payload,
        label: Option<&Label>,
        stamp: Stamp,
    ) {
        let outcome = match self.memory.merge(key, payload, label, stamp) {
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
