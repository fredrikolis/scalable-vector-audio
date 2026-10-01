// Concern: what a stream plays across its changes: graph, instances, keys, memory's answers, typing | Non-concern: the table, the blocks (stream.rs) | IO: (wanted nodes) -> a plan; commit, abort

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_ast::{Expr, Graph, Held};
use sva_formula::Hash;

use super::super::RenderConfig;
use super::super::frontier::{answers, noted};
use super::super::terms::{Handle, NOTES, Terms, is_term};
use super::STREAMED;
use crate::cache::{Known as Answer, Lookup, Outcome, Stored};
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::Instances;
use crate::schedule;
use crate::typing::Typing;

/// What memory answers a key with this round.
pub(super) type Found<'f> = &'f dyn Fn(Hash) -> Answer;

/// What a stream plays, each part changed in place and held once a change lands.
pub(super) struct World {
    pub(super) graph: Graph,
    pub(super) instances: Instances,
    pub(super) typing: Typing,
    known: BTreeMap<String, Known>,
    /// Each graph node the change tried set, and what it held before.
    edits: Vec<(String, Option<Held>)>,
    pub(super) own_notes: bool,
    config: RenderConfig,
    /// The instances a change tried in place of these, whole, set aside until it lands.
    replaced: Option<Instances>,
}

/// What one instance was last named and found as.
#[derive(Clone)]
struct Known {
    key: Hash,
    identity: Hash,
    group: Arc<[String]>,
    /// A loop's member, a term or a reader of the note sum: never looked up.
    pinned: bool,
    reads_notes: bool,
    looked: bool,
    stored: Option<Arc<Stored>>,
}

/// The nodes a change wants the stream to play.
pub(super) struct Wanted<'a> {
    pub(super) target: &'a Expr,
    pub(super) terms: &'a Terms,
    /// The term it adds, replaces or cuts, as it now stands.
    pub(super) term: Option<(Handle, &'a Expr)>,
    /// A graph reaching what it reads, and its reads.
    pub(super) from: Option<(&'a Graph, Vec<String>)>,
}

/// What a change named and found, and the groups it lowers, dependencies first.
pub(super) struct Plan {
    pub(super) adopted: usize,
    pub(super) named: usize,
    pub(super) visited: usize,
    pub(super) hits: Vec<Lookup>,
    pub(super) prefixes: BTreeMap<String, Arc<Stored>>,
    pub(super) groups: Vec<Vec<String>>,
    found: BTreeMap<String, Known>,
}

/// A plan, or the keys a walk has to look up before it can finish one.
pub(super) enum Walked {
    Asks(Vec<Hash>),
    Planned(Plan),
}

impl World {
    pub(super) fn new(graph: &Graph, config: &RenderConfig) -> Result<World, EngineError> {
        if graph.defines(STREAMED) {
            return Err(super::refusal(format!(
                "this composition already has a node named `{STREAMED}`"
            )));
        }
        Ok(World {
            graph: graph.clone(),
            instances: Instances::new(config.rate),
            typing: Typing::default(),
            known: BTreeMap::new(),
            edits: Vec::new(),
            own_notes: graph.defines(NOTES),
            config: config.clone(),
            replaced: None,
        })
    }

    /// The graph set to what `wanted` plays, what changed named and scanned, each instance
    /// it may rename keyed, and a walk to what the store answers of each changed or newly
    /// read. Undone on a refusal or keys to look up.
    pub(super) fn plan(&mut self, wanted: &Wanted, found: Found) -> Result<Walked, EngineError> {
        let planned = self.planned(wanted, found);
        match &planned {
            Ok(Walked::Planned(_)) => {}
            _ => {
                self.abort();
            }
        }
        planned
    }

    fn planned(&mut self, wanted: &Wanted, found: Found) -> Result<Walked, EngineError> {
        let adopted = match &wanted.from {
            Some((graph, roots)) => {
                self.renew(graph, roots);
                self.adopt(graph, roots)
            }
            None => 0,
        };
        let mut rewritten = Vec::new();
        self.set(STREAMED, wanted.target, &mut rewritten);
        if !wanted.terms.is_empty() && self.own_notes {
            return Err(EngineError::refused(Diagnostic {
                code: "engine.no_stream".to_string(),
                message: format!(
                    "a term is added to `@{NOTES}`, and this composition defines its own `{NOTES}`"
                ),
                location: Located::at(NOTES, None),
                help: format!(
                    "rename the composition's `{NOTES}`, or play it without adding terms"
                ),
            }));
        }
        let first = !self.instances.holds(STREAMED);
        if let Some((handle, term)) = wanted.term {
            self.set(&handle.node(), term, &mut rewritten);
        }
        if !self.own_notes {
            self.set(NOTES, &wanted.terms.sum(), &mut rewritten);
        }
        match first {
            true => self
                .instances
                .hold(&self.graph, &[STREAMED.to_string()])
                .map(|_| ())?,
            false => self.instances.rewrite(&self.graph, &rewritten)?,
        }
        let (named, rescanned) = self.instances.changing();
        let rescanned = rescanned.map(|(n, before)| (n.to_string(), before.to_vec()));
        let (named, rescanned): (Vec<String>, Vec<(String, Vec<String>)>) =
            (named.to_vec(), rescanned.collect());
        let region = self.region(&named, &rescanned);
        let (found_known, changed) = self.named(&region);
        let mut fresh: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for name in &named {
            let deps = self.instances.deps(name).iter().map(String::as_str);
            fresh.insert(name, deps.collect());
        }
        for (name, before) in &rescanned {
            let deps = self.instances.deps(name).iter();
            let new = deps.filter(|d| !before.contains(d)).map(String::as_str);
            fresh.insert(name, new.collect());
        }
        let walk = Walk {
            world: self,
            found_known,
            changed: &changed,
            fresh: &fresh,
            found,
        };
        let walked = walk.walked();
        let (mut found_known, visited, hits, asks) = walked;
        if !asks.is_empty() {
            return Ok(Walked::Asks(asks));
        }
        let groups = region
            .groups
            .into_iter()
            .filter(|group| group.iter().any(|path| changed.contains(path)))
            .collect();
        let prefixes = hits
            .iter()
            .filter_map(|hit| {
                let known = found_known.get_mut(&hit.node)?;
                Some((hit.node.clone(), Arc::clone(known.stored.as_ref()?)))
            })
            .collect();
        Ok(Walked::Planned(Plan {
            adopted,
            named: named.len(),
            visited,
            hits,
            prefixes,
            groups,
            found: found_known,
        }))
    }

    /// `path` defined as `expr` where the graph holds it otherwise.
    fn set(&mut self, path: &str, expr: &Expr, rewritten: &mut Vec<String>) {
        if self.graph.expr(path) == Some(expr) {
            return;
        }
        let held = self.graph.defining(expr.clone());
        let before = self.graph.set(path, Some(held));
        self.edits.push((path.to_string(), before));
        rewritten.push(path.to_string());
    }

    /// Each node played or read anew that `from` holds otherwise than the graph, taken into
    /// it; the instances are then named anew, whole, beside the ones set aside.
    fn renew(&mut self, from: &Graph, roots: &[String]) {
        let read = from.reaching(roots);
        let played = |p: &str| self.instances.instances_of(p).next().is_some() || read.contains(p);
        let paths = from.paths().filter(|p| self.graph.defines(p) && played(p));
        let changed: Vec<String> = paths
            .filter(|p| !self.graph.holds_as(p, from))
            .map(str::to_string)
            .collect();
        if changed.is_empty() {
            return;
        }
        for path in changed {
            let before = self.graph.set(&path, from.held(&path));
            self.edits.push((path, before));
        }
        let fresh = Instances::new(self.config.rate);
        self.replaced = Some(std::mem::replace(&mut self.instances, fresh));
    }

    fn adopt(&mut self, from: &Graph, roots: &[String]) -> usize {
        let taken = self.graph.adopt(from, roots);
        let count = taken.len();
        self.edits
            .extend(taken.into_iter().map(|path| (path, None)));
        count
    }

    /// What a change may rename: what it named and scanned again, their old groups and all
    /// reading them, grouped dependencies first.
    fn region(&self, named: &[String], rescanned: &[(String, Vec<String>)]) -> Region {
        let mut region: BTreeSet<String> = named.iter().cloned().collect();
        for (name, _) in rescanned {
            region.insert(name.clone());
            if let Some(known) = self.known.get(name) {
                region.extend(known.group.iter().cloned());
            }
        }
        let mut up: Vec<String> = region.iter().cloned().collect();
        while let Some(at) = up.pop() {
            for reader in self.instances.readers(&at) {
                if region.insert(reader.to_string()) {
                    up.push(reader.to_string());
                }
            }
        }
        let starts: Vec<String> = region.iter().cloned().collect();
        let groups = schedule::grouped(&self.instances, &starts, &|path| region.contains(path));
        Region { groups }
    }

    /// Each member of the region named, dependencies first, and those whose key moved.
    fn named(&self, region: &Region) -> (BTreeMap<String, Known>, BTreeSet<String>) {
        let mut found: BTreeMap<String, Known> = BTreeMap::new();
        let mut changed = BTreeSet::new();
        for group in &region.groups {
            let of = |read: &str| {
                let known = found.get(read).or_else(|| self.known.get(read));
                known.expect("a read named before its reader").identity
            };
            let named = crate::source::group_identities(&self.graph, &self.instances, group, &of);
            let looped = schedule::is_loop(&self.instances, group);
            let reads_notes = !self.own_notes
                && group.iter().any(|path| {
                    path == NOTES
                        || self.instances.deps(path).iter().any(|read| {
                            let known = found.get(read).or_else(|| self.known.get(read));
                            read == NOTES || known.is_some_and(|k| k.reads_notes)
                        })
                });
            let members: Arc<[String]> = group.clone().into();
            for (path, identity) in named {
                let key = crate::cache::node_key(identity, self.config.rate, &self.config.profile);
                let old = self.known.get(&path).filter(|old| old.key == key);
                if old.is_none() {
                    changed.insert(path.clone());
                }
                let pinned = reads_notes || looped || (!self.own_notes && is_term(&path));
                let known = Known {
                    key,
                    identity,
                    group: Arc::clone(&members),
                    pinned,
                    reads_notes,
                    looked: old.is_some_and(|old| old.looked),
                    stored: old.and_then(|old| old.stored.clone()),
                };
                found.insert(path, known);
            }
        }
        (found, changed)
    }

    /// The plan held, what nothing reads let go; the typing's freed ids.
    pub(super) fn commit(&mut self, plan: &mut Plan) -> Vec<sva_formula::NodeId> {
        let mut removed = self.instances.commit();
        if let Some(old) = self.replaced.take() {
            let gone = old.paths().filter(|p| !self.instances.holds(p));
            removed.extend(gone.map(str::to_string));
        }
        self.typing.hide(removed.iter().cloned());
        let freed = self.typing.commit(&self.instances);
        self.known.extend(std::mem::take(&mut plan.found));
        for path in &removed {
            self.known.remove(path);
            if is_term(path) && !self.own_notes {
                self.graph.set(path, None);
            }
        }
        self.edits.clear();
        freed
    }

    /// Everything since the last commit undone; the draft typing's ids.
    pub(super) fn abort(&mut self) -> Vec<sva_formula::NodeId> {
        let freed = self.typing.abort();
        self.instances.abort();
        if let Some(old) = self.replaced.take() {
            self.instances = old;
        }
        for (path, before) in std::mem::take(&mut self.edits).into_iter().rev() {
            self.graph.set(&path, before);
        }
        freed
    }
}

struct Region {
    groups: Vec<Vec<String>>,
}

/// A walk down through the store's misses to each node changed or newly read.
struct Walk<'w> {
    world: &'w World,
    found_known: BTreeMap<String, Known>,
    changed: &'w BTreeSet<String>,
    fresh: &'w BTreeMap<&'w str, BTreeSet<&'w str>>,
    found: Found<'w>,
}

type WalkedOut = (BTreeMap<String, Known>, usize, Vec<Lookup>, Vec<Hash>);

impl Walk<'_> {
    fn known(&self, path: &str) -> Known {
        let known = self
            .found_known
            .get(path)
            .or_else(|| self.world.known.get(path));
        known.cloned().expect("an instance named")
    }

    fn walked(mut self) -> WalkedOut {
        let (mut visited, mut hits, mut asks) = (BTreeSet::new(), Vec::new(), Vec::new());
        let mut stack = vec![(STREAMED.to_string(), false)];
        while let Some((path, anew)) = stack.pop() {
            let mut known = self.known(&path);
            if visited.contains(&path) {
                let asked = anew && !known.pinned && known.stored.is_none();
                if asked && matches!((self.found)(known.key), Answer::Unknown) {
                    asks.push(known.key);
                }
                continue;
            }
            let newly = !known.looked;
            if !(anew || newly || self.changed.contains(&path)) {
                continue;
            }
            visited.insert(path.clone());
            let stored = match known.pinned {
                true => None,
                false => match (self.found)(known.key) {
                    Answer::Hit(hit) => Some(hit),
                    Answer::Miss => None,
                    Answer::Unknown => {
                        asks.push(known.key);
                        continue;
                    }
                },
            };
            let config = &self.world.config;
            let stored = stored.filter(|hit| answers(hit, false, config));
            known.looked = true;
            known.stored = stored.clone();
            if stored.is_some() {
                hits.push(noted(&path, known.key, Outcome::Hit));
            } else {
                let fresh = self.fresh.get(path.as_str());
                for read in self.world.instances.deps(&path).iter().rev() {
                    let anew = fresh.is_some_and(|f| f.contains(read.as_str()));
                    stack.push((read.clone(), anew));
                }
            }
            self.found_known.insert(path, known);
        }
        asks.sort();
        asks.dedup();
        (self.found_known, visited.len(), hits, asks)
    }
}
