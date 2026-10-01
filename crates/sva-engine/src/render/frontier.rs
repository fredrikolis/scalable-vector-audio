// Concern: walks down from the root to the nodes memory answers, before any typing | Non-concern: naming a node, computing the rest | IO: (Tier, root) -> hits, visited nodes, lookups

use std::collections::{BTreeMap, BTreeSet};
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
            pinned: pinned(inst, order, config),
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
            let key = self.keys[&path];
            let found = match self.pinned.contains(&path) {
                false => match known(key) {
                    Known::Hit(hit) => Some(hit),
                    Known::Miss => None,
                    Known::Unknown => {
                        self.stack.push(Step::Visit(path));
                        return Some(key);
                    }
                },
                true => None,
            };
            self.visited.insert(path.clone());
            let root = path == self.root;
            if let Some(hit) = found.filter(|hit| answers(hit, root, self.config)) {
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

/// A node looked up nowhere: a loop's member, whose samples hold its own past, a node a reading
/// asks more of than its samples, and every node above one a reading asks of, which a hit would
/// hide; a ledger reads every node under its own.
fn pinned(inst: &Instances, order: &Order, config: &RenderConfig) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = order
        .groups
        .iter()
        .filter(|group| order.is_loop(group))
        .flatten()
        .cloned()
        .collect();
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
        if ask.representation != Representation::Samples {
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
