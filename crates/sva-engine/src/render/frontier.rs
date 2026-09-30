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

pub(crate) type Known = BTreeMap<Hash, Option<Arc<Stored>>>;

pub(crate) struct Frontier<'w> {
    order: &'w Order,
    keys: &'w BTreeMap<String, Hash>,
    root: &'w str,
    config: &'w RenderConfig,
    streaming: bool,
    pinned: BTreeSet<String>,
    /// Walked beneath a streamed hit, not looked up.
    unlooked: BTreeSet<String>,
    stack: Vec<Step>,
    pub(crate) stored: BTreeMap<String, Arc<Stored>>,
    pub(crate) visited: BTreeSet<String>,
    pub(crate) lookups: Vec<Lookup>,
}

/// Each flags a lookup: nothing beneath a streamed hit is looked up.
enum Step {
    Visit(String, bool),
    Close(String, bool),
}

impl<'w> Frontier<'w> {
    pub(crate) fn from(
        (inst, order): (&Instances<'_>, &'w Order),
        keys: &'w BTreeMap<String, Hash>,
        root: &'w str,
        (config, streaming): (&'w RenderConfig, bool),
    ) -> Frontier<'w> {
        Frontier {
            order,
            keys,
            root,
            config,
            streaming,
            pinned: pinned(inst, order, config),
            unlooked: BTreeSet::new(),
            stack: vec![Step::Visit(root.to_string(), true)],
            stored: BTreeMap::new(),
            visited: BTreeSet::new(),
            lookups: Vec::new(),
        }
    }

    /// A hit ends the walk down its branch, or streaming, the lookups; a miss walks on. It
    /// pauses at the first key `known` lacks, answering it.
    pub(crate) fn walk(&mut self, known: &Known) -> Option<Hash> {
        while let Some(step) = self.stack.pop() {
            let (path, look) = match step {
                Step::Close(path, looked) => {
                    if looked && !self.pinned.contains(&path) {
                        let key = self.keys[&path];
                        self.lookups
                            .push(noted(&path, key, Outcome::ComputedNotStored));
                    }
                    continue;
                }
                Step::Visit(path, look) => (path, look),
            };
            let relook = look && self.unlooked.contains(&path);
            if !relook && self.visited.contains(&path) {
                continue;
            }
            let key = self.keys[&path];
            let found = match look && !self.pinned.contains(&path) {
                true => match known.get(&key) {
                    Some(found) => found.clone(),
                    None => {
                        self.stack.push(Step::Visit(path, look));
                        return Some(key);
                    }
                },
                false => None,
            };
            self.unlooked.remove(&path);
            let late = !self.visited.insert(path.clone());
            if !look {
                self.unlooked.insert(path.clone());
            }
            let root = path == self.root && !self.streaming;
            if let Some(hit) = found.filter(|hit| answers(hit, root, self.config)) {
                self.lookups.push(noted(&path, key, Outcome::Hit));
                self.stored.insert(path.clone(), hit);
                if self.streaming && !late {
                    self.opened(path, false);
                }
                continue;
            }
            match late {
                true => self.relooked(path),
                false => self.opened(path, look),
            }
        }
        None
    }

    pub(crate) async fn walked(&mut self, known: &mut Known, store: &impl Through) {
        while let Some(key) = self.walk(known) {
            known.insert(key, store.lookup(key).await.map(Arc::new));
        }
    }

    /// Nodes no store can hold, never looked up.
    pub(crate) fn unstored(&mut self, paths: impl IntoIterator<Item = String>) {
        self.pinned.extend(paths);
    }

    /// A hit whose samples fall short of what its readers ask, walked on into as a miss.
    pub(crate) fn reopen(&mut self, path: &str) {
        self.stored.remove(path);
        self.lookups.retain(|lookup| lookup.node != path);
        self.opened(path.to_string(), true);
    }

    /// Walked unlooked, then missed: its reads are looked up.
    fn relooked(&mut self, path: String) {
        if !self.pinned.contains(&path) {
            let key = self.keys[&path];
            self.lookups
                .push(noted(&path, key, Outcome::ComputedNotStored));
        }
        for read in self.order.deps(&path).iter().rev() {
            self.stack.push(Step::Visit(read.clone(), true));
        }
    }

    fn opened(&mut self, path: String, look: bool) {
        let order = self.order;
        let reads = order.deps(&path);
        self.stack.push(Step::Close(path, look));
        for read in reads.iter().rev() {
            self.stack.push(Step::Visit(read.clone(), look));
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

/// A render's root answers where it holds the samples read; any other node, or a stream's
/// root, where a reader may take its samples.
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
