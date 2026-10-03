// Concern: walks down from the root to the nodes memory answers, each keyed by what it computes | Non-concern: naming a node, computing the rest | IO: (Tier, root) -> hits, visited nodes, lookups

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use sva_formula::Hash;
use sva_samples::Extent;

use super::{RenderConfig, default_end, default_start};
use crate::cache::{Backend, Known, Lookup, Outcome, PayloadKind, Stored, Tier};
use crate::instantiate::Instances;
use crate::query::Representation;
use crate::schedule::Order;

pub(crate) struct Frontier<'w> {
    order: &'w Order<'w>,
    keys: &'w BTreeMap<String, Hash>,
    root: &'w str,
    config: &'w RenderConfig,
    pinned: BTreeSet<String>,
    asked: BTreeMap<String, Vec<Representation>>,
    stack: Vec<Step>,
    pub(crate) stored: BTreeMap<String, Arc<Stored>>,
    pub(crate) visited: BTreeSet<String>,
    pub(crate) lookups: Vec<Lookup>,
}

enum Step {
    Visit(String),
    Close(String),
}

impl<'w> Frontier<'w> {
    pub(crate) fn from(
        (inst, order): (&Instances, &'w Order<'w>),
        keys: &'w BTreeMap<String, Hash>,
        root: &'w str,
        config: &'w RenderConfig,
    ) -> Frontier<'w> {
        Frontier {
            order,
            keys,
            root,
            config,
            pinned: pinned(inst, order, config, keys),
            asked: asked(inst, config),
            stack: vec![Step::Visit(root.to_string())],
            stored: BTreeMap::new(),
            visited: BTreeSet::new(),
            lookups: Vec::new(),
        }
    }

    /// A hit ends the walk down its branch; a miss walks on. It pauses at the first key
    /// `known` cannot answer, answering it.
    pub(crate) fn walk(&mut self, known: &dyn Fn(Hash) -> Known) -> Option<Hash> {
        while let Some(step) = self.stack.pop() {
            let path = match step {
                Step::Close(path) => {
                    if !self.pinned.contains(&path) {
                        let key = self.keys[&path];
                        self.lookups
                            .push(noted(&path, key, Outcome::ComputedNotStored));
                    }
                    continue;
                }
                Step::Visit(path) => path,
            };
            if self.visited.contains(&path) {
                continue;
            }
            let key = self.keys.get(&path).copied();
            let found = match key.filter(|_| !self.pinned.contains(&path)) {
                Some(key) => match known(key) {
                    Known::Hit(hit) => Some(hit),
                    Known::Miss => None,
                    Known::Unknown => {
                        self.stack.push(Step::Visit(path));
                        return Some(key);
                    }
                },
                None => None,
            };
            self.visited.insert(path.clone());
            let root = path == self.root;
            let read = self.asked.get(&path).map_or(&[][..], Vec::as_slice);
            let answering = |hit: &Arc<Stored>| {
                answers(hit, root, self.config) && read.iter().all(|r| r.off_samples(hit.sampled))
            };
            if let (Some(hit), Some(key)) = (found.filter(answering), key) {
                self.lookups.push(noted(&path, key, Outcome::Hit));
                self.stored.insert(path.clone(), hit);
                continue;
            }
            self.opened(path);
        }
        None
    }

    /// Walked to its end, each key memory cannot answer looked up through it this round.
    pub(crate) async fn walked<B: Backend>(&mut self, tier: &Tier<B>, round: u64) {
        loop {
            let memory = tier.memory();
            let Some(key) = self.walk(&|key| memory.answer(key, round)) else {
                return;
            };
            tier.lookup(key, round).await;
        }
    }

    /// A hit whose samples fall short of what its readers ask, walked on into as a miss.
    pub(crate) fn reopen(&mut self, path: &str) {
        self.stored.remove(path);
        self.lookups.retain(|lookup| lookup.node != path);
        self.opened(path.to_string());
    }

    /// Each node's `own` with that of every node under it: one fold over the walk's groups,
    /// dependencies first, a node adding nothing sharing the set it reads.
    pub(crate) fn beneath<'o, T: Ord + Copy + 'o>(
        &self,
        own: &dyn Fn(&str) -> Option<&'o [T]>,
    ) -> HashMap<&'w str, Rc<BTreeSet<T>>> {
        let order: &'w Order<'w> = self.order;
        let mut out: HashMap<&'w str, Rc<BTreeSet<T>>> = HashMap::new();
        for group in &order.groups {
            let inside = |dep: &String| group.contains(dep);
            let mut sets: Vec<Rc<BTreeSet<T>>> = Vec::new();
            for member in group {
                let read = order.deps(member).iter().filter(|d| !inside(d));
                for held in read.filter_map(|dep| out.get(dep.as_str())) {
                    if !sets.iter().any(|set| Rc::ptr_eq(set, held)) {
                        sets.push(Rc::clone(held));
                    }
                }
            }
            let mine = group.iter().filter_map(|m| own(m)).flatten();
            let set = match (sets.len(), mine.clone().next()) {
                (0, None) => Rc::default(),
                (1, None) => Rc::clone(&sets[0]),
                _ => {
                    crate::steps::step(sets.iter().map(|set| set.len()).sum());
                    let all = sets.iter().flat_map(|set| set.iter()).copied();
                    Rc::new(all.chain(mine.copied()).collect())
                }
            };
            for member in group {
                crate::steps::step(1);
                out.insert(member.as_str(), Rc::clone(&set));
            }
        }
        out
    }

    fn opened(&mut self, path: String) {
        let order = self.order;
        let reads = order.deps(&path);
        self.stack.push(Step::Close(path));
        for read in reads.iter().rev() {
            self.stack.push(Step::Visit(read.clone()));
        }
    }
}

pub(crate) fn noted(path: &str, key: Hash, outcome: Outcome) -> Lookup {
    Lookup {
        node: path.to_string(),
        key,
        kind: PayloadKind::Segments,
        outcome,
        store: Some(outcome == Outcome::Hit),
    }
}

/// A render's root answers where it holds the samples read; any other node, or a stream's
/// node, where a reader may take its samples.
pub(crate) fn answers(hit: &Stored, root: bool, config: &RenderConfig) -> bool {
    let support = hit.support;
    if !root {
        return hit.readable;
    }
    let start = config.range.start.unwrap_or_else(|| default_start(support));
    let Some(end) = config.range.end.or(default_end(support)) else {
        return false;
    };
    hit.holds(Extent::new(start, end.max(start)).intersect(support))
}

/// A node looked up nowhere: one with no key, a loop's member, whose samples hold its own past,
/// a node a reading asks more of than samples, and every node above one a reading asks of,
/// which a hit would hide; a ledger reads every node under its own.
fn pinned(
    inst: &Instances,
    order: &Order,
    config: &RenderConfig,
    keys: &BTreeMap<String, Hash>,
) -> BTreeSet<String> {
    let groups = order.groups.iter();
    let looped = groups.filter(|group| order.is_loop(group)).flatten();
    let unkeyed = order
        .groups
        .iter()
        .flatten()
        .filter(|path| !keys.contains_key(*path));
    let mut out: BTreeSet<String> = looped.chain(unkeyed).cloned().collect();
    let ledger = config
        .asks
        .iter()
        .any(|ask| matches!(ask.representation, Representation::Ledger { .. }));
    if ledger {
        out.extend(order.groups.iter().flatten().cloned());
    }
    let mut asked = BTreeSet::new();
    for ask in &config.asks {
        let Ok(path) = inst.instance_of(&ask.node) else {
            continue;
        };
        if !ask.representation.off_samples(true) {
            out.insert(path.clone());
        }
        asked.insert(path);
    }
    for path in order.groups.iter().flatten() {
        if order.deps(path).iter().any(|read| asked.contains(read)) {
            asked.insert(path.clone());
            out.insert(path.clone());
        }
    }
    out
}

/// The readings asked of each node: a hit answers them only where its samples alone do.
fn asked(inst: &Instances, config: &RenderConfig) -> BTreeMap<String, Vec<Representation>> {
    let mut out: BTreeMap<String, Vec<Representation>> = BTreeMap::new();
    for ask in &config.asks {
        if let Ok(path) = inst.instance_of(&ask.node) {
            out.entry(path).or_default().push(ask.representation);
        }
    }
    out
}
