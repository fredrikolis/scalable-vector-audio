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
    /// The disk answered it for the call that looked it up.
    pub from_store: bool,
}

/// Lookups by what each came to. An extended value was found short and written again, so it is
/// both a miss and new.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub hit: usize,
    /// The hits the disk answered; the rest memory held.
    pub from_store: usize,
    pub miss: usize,
    pub prefix: usize,
    pub new: usize,
    pub reused: usize,
}

impl Tally {
    pub fn of<'l>(lookups: impl IntoIterator<Item = &'l Lookup>) -> Tally {
        let mut tally = Tally::default();
        for lookup in lookups {
            tally = tally.plus(Tally::one(lookup));
        }
        tally
    }

    fn one(lookup: &Lookup) -> Tally {
        let outcome = lookup.outcome;
        let is = |held: bool| usize::from(held);
        Tally {
            hit: is(outcome == Outcome::Hit),
            from_store: is(outcome == Outcome::Hit && lookup.from_store),
            prefix: is(outcome == Outcome::Prefix),
            reused: is(outcome == Outcome::Reused),
            miss: is(!matches!(
                outcome,
                Outcome::Hit | Outcome::Prefix | Outcome::Reused
            )),
            new: is(matches!(
                outcome,
                Outcome::ComputedStored | Outcome::ComputedReplaced | Outcome::Extended
            )),
        }
    }

    pub fn looked(&self) -> usize {
        self.hit + self.miss + self.prefix
    }

    pub fn plus(self, more: Tally) -> Tally {
        self.each(more, usize::wrapping_add)
    }

    /// Wrapping, so a tally that fell meanwhile adds back as the fall.
    pub fn minus(self, less: Tally) -> Tally {
        self.each(less, usize::wrapping_sub)
    }

    fn each(self, other: Tally, by: fn(usize, usize) -> usize) -> Tally {
        Tally {
            hit: by(self.hit, other.hit),
            from_store: by(self.from_store, other.from_store),
            miss: by(self.miss, other.miss),
            prefix: by(self.prefix, other.prefix),
            new: by(self.new, other.new),
            reused: by(self.reused, other.reused),
        }
    }
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
    /// Supports found by analysis: none a stored node told.
    pub analysed: usize,
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

    pub fn tally(&self) -> Tally {
        Tally::of(&self.lookups)
    }

    fn count(&self, of: impl Fn(Outcome) -> bool) -> usize {
        self.lookups.iter().filter(|l| of(l.outcome)).count()
    }
}

/// What the memo told one render or stream: one lookup per key a change asks.
pub(crate) struct Recording {
    since: Counters,
    /// Memory's clock where it began.
    began: u64,
    failures: u64,
    lookups: Recent<Lookup>,
    shed: Tally,
    reached: Option<Vec<(i64, usize)>>,
    /// Each key's latest lookup still held, and the change that made it.
    looked: HashMap<Hash, (u64, usize)>,
    change: u64,
}

impl Recording {
    pub(crate) fn over(memory: &Memory) -> Recording {
        Recording {
            since: memory.counters(),
            began: memory.clock(),
            failures: memory.failures().0,
            lookups: Recent::keeping(usize::MAX),
            shed: Tally::default(),
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
            analysed: 0,
            unstaged: why.filter(|_| failed || memory.blocked()),
            unslotted: None,
        }
    }

    pub(crate) fn began(&self) -> u64 {
        self.began
    }

    /// Whether the disk answered `key`'s lookup.
    pub(crate) fn stored(&self, key: Hash) -> bool {
        let lookup = self.looked(key).and_then(|(_, at)| self.lookups.get(at));
        lookup.is_some_and(|lookup| lookup.from_store)
    }

    pub(crate) fn since(&self, memory: &Memory) -> Counters {
        memory.counters().since(self.since)
    }

    pub(crate) fn tally(&self) -> Tally {
        self.shed.plus(Tally::of(self.lookups.iter()))
    }

    fn made(&mut self, lookup: Lookup) -> usize {
        let (at, shed) = self.lookups.pushed(lookup);
        if let Some(shed) = shed {
            self.shed = self.shed.plus(Tally::one(&shed));
        }
        at
    }

    /// Asked again in one change, a key's lookup is settled by what any answer found.
    pub(super) fn answered(
        &mut self,
        (key, node): (Hash, &str),
        kind: PayloadKind,
        (outcome, from_store): (Outcome, bool),
    ) {
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
                (lookup.outcome, lookup.from_store) = (outcome, from_store);
            }
            return;
        }
        let lookup = Lookup {
            node: node.to_string(),
            key,
            kind,
            outcome,
            from_store,
        };
        let at = self.made(lookup);
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
        self.made(Lookup {
            node: node.to_string(),
            key,
            kind,
            outcome: Outcome::Reused,
            from_store: false,
        });
    }

    fn looked(&self, key: Hash) -> Option<(u64, usize)> {
        self.looked.get(&key).copied()
    }
}
