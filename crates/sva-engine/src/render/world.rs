// Concern: the version a render or stream plays, advanced per edit, and its walk to what memory answers | Non-concern: the table, the blocks | IO: (an edit) -> a version; (a root) -> hits; commit, abort

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_ast::{Expr, Graph, Held};
use sva_formula::Hash;

use sva_samples::Extent;

use super::terms::{Handle, NOTES, Terms, is_term};
use super::{RenderConfig, default_end, default_start};
use crate::cache::{Known as Answer, Lookup, Outcome, PayloadKind, Stored};
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::Instances;
use crate::query::Representation;
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
        if self.notes {
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
        let walking = Walking {
            root: STREAMED,
            config,
            whole: false,
            opened: &BTreeSet::new(),
        };
        let reached = match self.reach(&walking, Some(&advance), found) {
            Reach::Asks(keys) => return Walked::Asks(keys),
            Reach::Reached(reached) => reached,
        };
        let hits = reached.lookups.into_iter();
        Walked::Planned(Plan {
            adopted: advance.adopted,
            named: advance.named.len(),
            visited: reached.visited.len(),
            hits: hits.filter(|l| l.outcome == Outcome::Hit).collect(),
            prefixes: reached.held,
            found: reached.known,
        })
    }

    /// A walk down from `walking`'s root to what memory answers, through every miss: a
    /// render's whole, or a change's to each node it changed or newly reads.
    pub(super) fn walk(&self, walking: &Walking, found: Found) -> Reach {
        self.reach(walking, None, found)
    }

    fn reach(&self, walking: &Walking, change: Option<&Advance>, found: Found) -> Reach {
        let walk = Walk {
            world: self,
            known: change.map_or_else(BTreeMap::new, |c| c.found.clone()),
            changed: change.map(|c| &c.changed),
            fresh: change.map(|c| &c.fresh),
            pins: Pins::of(&self.instances, walking.config),
            walking,
            found,
        };
        walk.walked()
    }

    pub(super) fn key(&self, path: &str, config: &RenderConfig) -> Option<Hash> {
        self.keyed(path, self.known.get(path)?, config)
    }

    /// What memory holds `known`'s own value under; none where its identity refuses.
    fn keyed(&self, path: &str, known: &Known, config: &RenderConfig) -> Option<Hash> {
        let id = self.typing.id(path)?;
        let identity = known.identity?;
        Some(super::table::node_key(
            &self.typing,
            id,
            identity,
            &config.profile,
        ))
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

/// How a walk asks memory: from which root, at which rate and profile, for which readings.
pub(super) struct Walking<'w> {
    pub(super) root: &'w str,
    pub(super) config: &'w RenderConfig,
    /// A render's: every node from the root looked up anew, the root answering over its range.
    pub(super) whole: bool,
    /// Hits short of what their readers ask, walked into as misses.
    pub(super) opened: &'w BTreeSet<String>,
}

/// What a walk reached: each node visited, each lookup in walk order, each hit's samples.
pub(super) struct Reached {
    pub(super) visited: BTreeSet<String>,
    pub(super) lookups: Vec<Lookup>,
    pub(super) held: BTreeMap<String, Arc<Stored>>,
    known: BTreeMap<String, Known>,
}

/// What a walk reached, or the keys it has to look up before it can.
pub(super) enum Reach {
    Asks(Vec<Hash>),
    Reached(Reached),
}

/// What the readings asked of a walk pin: every node under a ledger, a node a reading asks
/// more of than samples, and every node above one a reading asks of, which a hit would hide.
struct Pins {
    all: bool,
    pinned: BTreeSet<String>,
    asked: BTreeMap<String, Vec<Representation>>,
}

impl Pins {
    fn of(inst: &Instances, config: &RenderConfig) -> Pins {
        let ledger =
            |ask: &&crate::query::Ask| matches!(ask.representation, Representation::Ledger { .. });
        let mut pins = Pins {
            all: config.asks.iter().any(|ask| ledger(&ask)),
            pinned: BTreeSet::new(),
            asked: BTreeMap::new(),
        };
        for ask in &config.asks {
            let Ok(path) = inst.instance_of(&ask.node) else {
                continue;
            };
            if !ask.representation.off_samples(true) {
                pins.pinned.insert(path.clone());
            }
            pins.asked.entry(path).or_default().push(ask.representation);
        }
        let mut up: Vec<&str> = pins.asked.keys().map(String::as_str).collect();
        while let Some(at) = up.pop() {
            for reader in inst.readers(at) {
                if pins.pinned.insert(reader.to_string()) {
                    up.push(reader);
                }
            }
        }
        pins
    }

    fn holds(&self, path: &str) -> bool {
        self.all || self.pinned.contains(path)
    }
}

struct Walk<'w> {
    world: &'w World,
    known: BTreeMap<String, Known>,
    changed: Option<&'w BTreeSet<String>>,
    fresh: Option<&'w BTreeMap<String, BTreeSet<String>>>,
    pins: Pins,
    walking: &'w Walking<'w>,
    found: Found<'w>,
}

enum Step {
    Visit(String, bool),
    Close(String, Hash),
}

impl Walk<'_> {
    fn known(&self, path: &str) -> Known {
        let known = self.known.get(path).or_else(|| self.world.known.get(path));
        known.cloned().expect("an instance named")
    }

    fn walked(mut self) -> Reach {
        let (mut visited, mut lookups, mut asks) = (BTreeSet::new(), Vec::new(), Vec::new());
        let mut held: BTreeMap<String, Arc<Stored>> = BTreeMap::new();
        let (config, whole) = (self.walking.config, self.walking.whole);
        let mut stack = vec![Step::Visit(self.walking.root.to_string(), false)];
        while let Some(step) = stack.pop() {
            let (path, anew) = match step {
                Step::Close(path, key) => {
                    lookups.push(noted(&path, key, Outcome::ComputedNotStored));
                    continue;
                }
                Step::Visit(path, anew) => (path, anew),
            };
            let mut known = self.known(&path);
            let pinned = known.pinned || self.pins.holds(&path);
            let key = self.world.keyed(&path, &known, config).filter(|_| !pinned);
            if visited.contains(&path) {
                let asked = anew && !held.contains_key(&path);
                if let Some(key) = key.filter(|_| asked)
                    && matches!((self.found)(key), Answer::Unknown)
                {
                    asks.push(key);
                }
                continue;
            }
            let changed = self.changed.is_some_and(|c| c.contains(&path));
            if !(whole || anew || !known.looked || changed) {
                continue;
            }
            visited.insert(path.clone());
            let looked = key.filter(|_| !self.walking.opened.contains(&path));
            let stored = match looked {
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
            let root = whole && path == self.walking.root;
            let read = self.pins.asked.get(&path).map_or(&[][..], Vec::as_slice);
            let answering = |hit: &Arc<Stored>| {
                answers(hit, root, config) && read.iter().all(|r| r.off_samples(hit.sampled))
            };
            known.looked = true;
            match stored.filter(|(_, hit)| answering(hit)) {
                Some((key, stored)) => {
                    lookups.push(noted(&path, key, Outcome::Hit));
                    held.insert(path.clone(), stored);
                }
                None => {
                    if let Some(key) = key {
                        stack.push(Step::Close(path.clone(), key));
                    }
                    let fresh = self.fresh.and_then(|f| f.get(&path));
                    for read in self.world.instances.deps(&path).iter().rev() {
                        let anew = fresh.is_some_and(|f| f.contains(read));
                        stack.push(Step::Visit(read.clone(), anew));
                    }
                }
            }
            self.known.insert(path, known);
        }
        if !asks.is_empty() {
            asks.sort();
            asks.dedup();
            return Reach::Asks(asks);
        }
        Reach::Reached(Reached {
            visited,
            lookups,
            held,
            known: self.known,
        })
    }
}

pub(super) fn noted(path: &str, key: Hash, outcome: Outcome) -> Lookup {
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
