// Concern: the version a render or stream plays, advanced per edit: graph, instances, typing, identities | Non-concern: the table, the blocks | IO: (an edit) -> a version, a walk; commit, abort

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_ast::{Expr, Graph, Held};
use sva_formula::Hash;

use super::RenderConfig;
use super::frontier::{answers, noted};
use super::terms::{Handle, NOTES, Terms, is_term};
use crate::cache::{Known as Answer, Lookup, Outcome, Stored};
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::Instances;
use crate::schedule;
use crate::typing::Typing;

pub const STREAMED: &str = "streamed";

/// What memory answers a key with this round.
pub(super) type Found<'f> = &'f dyn Fn(Hash) -> Answer;

pub(super) struct World {
    pub(super) graph: Graph,
    pub(super) instances: Instances,
    pub(super) typing: Typing,
    known: BTreeMap<String, Known>,
    edits: Vec<(String, Option<Held>)>,
    /// Whether it writes `@notes` as the sum of its terms: a stream's, over a composition that
    /// defines none.
    pub(super) notes: bool,
    replaced: Option<Instances>,
}

#[derive(Clone)]
pub(super) struct Known {
    /// What it computes; none where its identity refuses.
    identity: Option<Hash>,
    group: Arc<[String]>,
    /// A loop's member, a term, a reader of the note sum or one with no identity: never looked up.
    pinned: bool,
    reads_notes: bool,
    looked: bool,
}

impl Known {
    fn key(&self, config: &RenderConfig) -> Option<Hash> {
        let identity = self.identity?;
        Some(crate::cache::node_key(
            identity,
            config.rate,
            &config.profile,
        ))
    }
}

/// The node a version plays: a stream's expression, held as its own node, or a node of the graph.
pub(super) enum Root<'a> {
    Streamed(&'a Expr),
    Node(&'a str),
}

pub(super) struct Wanted<'a> {
    pub(super) root: Root<'a>,
    pub(super) terms: &'a Terms,
    /// The term it adds, replaces or cuts, as it now stands.
    pub(super) term: Option<(Handle, &'a Expr)>,
    /// A graph reaching what it reads, and its reads.
    pub(super) from: Option<(&'a Graph, Vec<String>)>,
    /// Whether `from` holds all there is, so a node it lacks is gone.
    pub(super) whole: bool,
}

pub(super) struct Advance {
    adopted: usize,
    named: Vec<String>,
    found: BTreeMap<String, Known>,
    changed: BTreeSet<String>,
    fresh: BTreeMap<String, BTreeSet<String>>,
}

pub(super) struct Plan {
    pub(super) adopted: usize,
    pub(super) named: usize,
    pub(super) visited: usize,
    pub(super) hits: Vec<Lookup>,
    pub(super) prefixes: BTreeMap<String, Arc<Stored>>,
    pub(super) found: BTreeMap<String, Known>,
}

/// A plan, or the keys a walk has to look up before it can finish one.
pub(super) enum Walked {
    Asks(Vec<Hash>),
    Planned(Plan),
}

impl World {
    /// `graph` as it stands, played at `rate`; `notes` where it writes `@notes` from its terms.
    pub(super) fn over(graph: &Graph, rate: u32, notes: bool) -> World {
        World {
            graph: graph.clone(),
            instances: Instances::new(rate),
            typing: Typing::default(),
            known: BTreeMap::new(),
            edits: Vec::new(),
            notes,
            replaced: None,
        }
    }

    /// `target` of `graph`, at `rate`, the version in `world` advanced to it and held: only
    /// what changed since, and its readers, typed anew. Its root instance; on a refusal the
    /// version is as it was.
    pub(super) fn rendered<'w>(
        world: &'w mut Option<World>,
        graph: &Graph,
        target: &str,
        rate: u32,
    ) -> Result<(&'w World, String), EngineError> {
        let world = match world.take() {
            Some(held) if held.instances.rate() == rate => world.insert(held),
            _ => world.insert(World::over(graph, rate, false)),
        };
        let wanted = Wanted {
            root: Root::Node(target),
            terms: &Terms::default(),
            term: None,
            from: Some((graph, vec![target.to_string()])),
            whole: true,
        };
        match world.advanced(&wanted) {
            Ok(advance) => {
                world.commit(advance.found);
                let root = world.instances.instance_of(target)?;
                Ok((world, root))
            }
            Err(refused) => {
                world.abort();
                Err(refused)
            }
        }
    }

    /// The graph set to what `wanted` plays, what changed named, scanned and typed, each
    /// instance it may rename named by what it computes, and a walk to what the store answers
    /// of each changed or newly read. Undone on a refusal or keys to look up.
    pub(super) fn plan(
        &mut self,
        wanted: &Wanted,
        config: &RenderConfig,
        found: Found,
    ) -> Result<Walked, EngineError> {
        let planned = self
            .advanced(wanted)
            .map(|advance| self.walked(advance, config, found));
        match &planned {
            Ok(Walked::Planned(_)) => {}
            _ => {
                self.abort();
            }
        }
        planned
    }

    fn advanced(&mut self, wanted: &Wanted) -> Result<Advance, EngineError> {
        let root = match wanted.root {
            Root::Streamed(_) => STREAMED,
            Root::Node(path) => path,
        };
        let held: Vec<&String> = self.instances.own_terms.keys().collect();
        let moved = !held.is_empty() && held != [root];
        let adopted = match &wanted.from {
            Some((graph, roots)) => {
                self.renew(graph, roots, (moved, wanted.whole));
                self.adopt(graph, roots)
            }
            None => 0,
        };
        let mut rewritten = Vec::new();
        if let Root::Streamed(target) = wanted.root {
            self.set(STREAMED, target, &mut rewritten);
        }
        if !wanted.terms.is_empty() && !self.notes {
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
        let first = !self.instances.own_terms.contains_key(root);
        if let Some((handle, term)) = wanted.term {
            self.set(&handle.node(), term, &mut rewritten);
        }
        if self.notes {
            self.set(NOTES, &wanted.terms.sum(), &mut rewritten);
        }
        match first {
            true => self
                .instances
                .hold(&self.graph, &[root.to_string()])
                .map(|_| ())?,
            false => self.instances.rewrite(&self.graph, &rewritten)?,
        }
        let (named, rescanned) = self.instances.changing();
        let rescanned = rescanned.map(|(n, before)| (n.to_string(), before.to_vec()));
        let (mut named, rescanned): (Vec<String>, Vec<(String, Vec<String>)>) =
            (named.to_vec(), rescanned.collect());
        if let Some(old) = &self.replaced {
            let held = |path: &String| old.resolution(path) == self.instances.resolution(path);
            named.retain(|path| !held(path));
        }
        let region = self.region(&named, &rescanned);
        self.typing.lower(&self.instances, &region)?;
        if let Root::Streamed(_) = wanted.root {
            wanted.terms.name(&mut self.typing);
        }
        let (found, changed) = self.named(&region);
        let mut fresh: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for name in &named {
            let deps = self.instances.deps(name).iter().cloned();
            fresh.insert(name.clone(), deps.collect());
        }
        for (name, before) in &rescanned {
            let deps = self.instances.deps(name).iter();
            let new = deps.filter(|d| !before.contains(d)).cloned();
            fresh.insert(name.clone(), new.collect());
        }
        Ok(Advance {
            adopted,
            named,
            found,
            changed,
            fresh,
        })
    }

    fn walked(&self, advance: Advance, config: &RenderConfig, found: Found) -> Walked {
        let walk = Walk {
            world: self,
            found_known: advance.found,
            changed: &advance.changed,
            fresh: &advance.fresh,
            config,
            found,
        };
        let (found_known, visited, hits, asks, prefixes) = walk.walked();
        if !asks.is_empty() {
            return Walked::Asks(asks);
        }
        Walked::Planned(Plan {
            adopted: advance.adopted,
            named: advance.named.len(),
            visited,
            hits,
            prefixes,
            found: found_known,
        })
    }

    pub(super) fn key(&self, path: &str, config: &RenderConfig) -> Option<Hash> {
        self.known.get(path)?.key(config)
    }

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
    /// it; where a parse moved, or the root `moved`, the instances are named anew, whole,
    /// beside the ones set aside.
    fn renew(&mut self, from: &Graph, roots: &[String], (moved, whole): (bool, bool)) {
        let read = from.reaching(roots);
        let played = |p: &str| self.instances.instances_of(p).next().is_some() || read.contains(p);
        let gone = self.graph.paths().filter(|p| whole && !from.defines(p));
        let paths = from.paths().filter(|p| self.graph.defines(p)).chain(gone);
        let held = |p: &&str| self.graph.holds_as(p, from) && self.graph.read_as(p, from);
        let paths = paths.filter(|p| played(p) && !held(p));
        let taken: Vec<String> = paths.map(str::to_string).collect();
        let reparsed = taken.iter().any(|p| !self.graph.holds_as(p, from));
        for path in taken {
            let before = self.graph.set(&path, from.held(&path));
            self.edits.push((path, before));
        }
        if !reparsed && !moved {
            return;
        }
        let fresh = Instances::new(self.instances.rate());
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
    fn region(&self, named: &[String], rescanned: &[(String, Vec<String>)]) -> Vec<Vec<String>> {
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
        schedule::grouped(&self.instances, &starts, &|path| region.contains(path))
    }

    /// Each member of the region named by what its typing computes, and those whose identity
    /// moved.
    fn named(&self, region: &[Vec<String>]) -> (BTreeMap<String, Known>, BTreeSet<String>) {
        let mut found: BTreeMap<String, Known> = BTreeMap::new();
        let mut changed = BTreeSet::new();
        for group in region {
            let looped = schedule::is_loop(&self.instances, group);
            let reads_notes = self.notes
                && group.iter().any(|path| {
                    path == NOTES
                        || self.instances.deps(path).iter().any(|read| {
                            let known = found.get(read).or_else(|| self.known.get(read));
                            read == NOTES || known.is_some_and(|k| k.reads_notes)
                        })
                });
            let members: Arc<[String]> = group.clone().into();
            for path in group {
                let typed = self.typing.id(path);
                let identity = typed.and_then(|id| crate::refs::identity(&self.typing, id).ok());
                let old = self
                    .known
                    .get(path)
                    .filter(|old| identity.is_some() && old.identity == identity);
                if old.is_none() {
                    changed.insert(path.clone());
                }
                let pinned =
                    identity.is_none() || reads_notes || looped || (self.notes && is_term(path));
                let known = Known {
                    identity,
                    group: Arc::clone(&members),
                    pinned,
                    reads_notes,
                    looked: old.is_some_and(|old| old.looked),
                };
                found.insert(path.clone(), known);
            }
        }
        (found, changed)
    }

    /// What nothing reads let go; the typing's freed ids.
    pub(super) fn commit(&mut self, found: BTreeMap<String, Known>) -> Vec<sva_formula::NodeId> {
        let mut removed = self.instances.commit();
        if let Some(old) = self.replaced.take() {
            let gone = old.paths().filter(|p| !self.instances.holds(p));
            removed.extend(gone.map(str::to_string));
        }
        self.typing.hide(removed.iter().cloned());
        let freed = self.typing.commit(&self.instances);
        self.known.extend(found);
        for path in &removed {
            self.known.remove(path);
            if is_term(path) && self.notes {
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

struct Walk<'w> {
    world: &'w World,
    found_known: BTreeMap<String, Known>,
    changed: &'w BTreeSet<String>,
    fresh: &'w BTreeMap<String, BTreeSet<String>>,
    config: &'w RenderConfig,
    found: Found<'w>,
}

type WalkedOut = (
    BTreeMap<String, Known>,
    usize,
    Vec<Lookup>,
    Vec<Hash>,
    BTreeMap<String, Arc<Stored>>,
);

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
        let mut held: BTreeMap<String, Arc<Stored>> = BTreeMap::new();
        let mut stack = vec![(STREAMED.to_string(), false)];
        let key = |known: &Known| known.key(self.config).filter(|_| !known.pinned);
        while let Some((path, anew)) = stack.pop() {
            let mut known = self.known(&path);
            if visited.contains(&path) {
                let asked = anew && !held.contains_key(&path);
                if let Some(key) = key(&known).filter(|_| asked)
                    && matches!((self.found)(key), Answer::Unknown)
                {
                    asks.push(key);
                }
                continue;
            }
            let newly = !known.looked;
            if !(anew || newly || self.changed.contains(&path)) {
                continue;
            }
            visited.insert(path.clone());
            let stored = match key(&known) {
                None => None,
                Some(key) => match (self.found)(key) {
                    Answer::Hit(hit) => Some((key, hit)),
                    Answer::Miss => None,
                    Answer::Unknown => {
                        asks.push(key);
                        continue;
                    }
                },
            };
            let stored = stored.filter(|(_, hit)| answers(hit, false, self.config));
            known.looked = true;
            if let Some((key, stored)) = stored {
                hits.push(noted(&path, key, Outcome::Hit));
                held.insert(path.clone(), stored);
            } else {
                let fresh = self.fresh.get(&path);
                for read in self.world.instances.deps(&path).iter().rev() {
                    let anew = fresh.is_some_and(|f| f.contains(read));
                    stack.push((read.clone(), anew));
                }
            }
            self.found_known.insert(path, known);
        }
        asks.sort();
        asks.dedup();
        (self.found_known, visited.len(), hits, asks, held)
    }
}
