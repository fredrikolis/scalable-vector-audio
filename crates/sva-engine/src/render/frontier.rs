// Concern: walks down from the root to the nodes the store answers, before any typing | Non-concern: naming a node, computing the rest | IO: (Store, root) -> hits, visited nodes, lookups

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_formula::Hash;
use sva_samples::Extent;

use super::{RenderConfig, default_end, default_start};
use crate::cache::{Lookup, Outcome, PayloadKind, Stored, Through};
use crate::instantiate::Instances;
use crate::query::Representation;
use crate::schedule::Order;

pub(crate) struct Frontier<'w> {
    order: &'w Order,
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
        (inst, order): (&Instances<'_>, &'w Order),
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

    /// A hit ends the walk down its branch; a miss goes on into every node it reads.
    pub(crate) async fn walk(&mut self, store: &impl Through) {
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
                Step::Visit(path) if self.visited.contains(&path) => continue,
                Step::Visit(path) => path,
            };
            self.visited.insert(path.clone());
            if !self.pinned.contains(&path)
                && let Some(hit) = store.lookup(self.keys[&path]).await
                && answers(&hit, path == self.root, self.config)
            {
                self.lookups
                    .push(noted(&path, self.keys[&path], Outcome::Hit));
                self.stored.insert(path, Arc::new(hit));
                continue;
            }
            self.opened(path);
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

fn noted(path: &str, key: Hash, outcome: Outcome) -> Lookup {
    Lookup {
        node: path.to_string(),
        key,
        kind: PayloadKind::Segments,
        outcome,
        store: Some(outcome == Outcome::Hit),
    }
}

/// The root answers where it holds the samples read; any other node where a reader may take
/// its samples, and planning its readers finds whether they hold all they ask.
fn answers(hit: &Stored, root: bool, config: &RenderConfig) -> bool {
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

/// A node looked up nowhere: a loop's member, whose samples hold its own past, and a node a
/// reading asks more of than its samples; a ledger reads every node under its own.
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
    for ask in &config.asks {
        if ask.representation != Representation::Samples
            && let Ok(path) = inst.instance_of(&ask.node)
        {
            out.insert(path);
        }
    }
    out
}
