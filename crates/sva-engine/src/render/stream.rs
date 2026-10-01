// Concern: opens a target as a stream over a store, editing it and its terms live | Non-concern: pulling its blocks, what an edit carries on | IO: (Graph, target) -> Stream; (Change) -> Changed

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_ast::{Expr, Graph};
use sva_formula::{Hash, NodeId};
use sva_samples::{Buffer, Extent};

use super::drive::{Block, Driver};
use super::frontier::Known as Met;
use super::table::Table;
use super::table::support::Supports;
use super::terms::{Handle, NOTES, Terms, cut, placed};
use super::{Ends, RenderConfig, range_over, through};
use crate::cache::{Cache, CacheStats, Recording, Stored, Through};
use crate::error::{Diagnostic, EngineError, Located};
use crate::flops::Work;
use crate::recent::Recent;
use world::{Plan, Walked, Wanted, World};

#[cfg(test)]
mod rebuilt;
mod world;

pub const STREAMED: &str = "streamed";

/// The lookups, and the names started silent, a stream keeps of all it made.
pub const LATEST: usize = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct StreamConfig {
    pub block: usize,
    /// The target's own at open where `None`; a mono one plays in each, a wider is refused.
    pub channels: Option<usize>,
    pub render: RenderConfig,
}

/// A target rendered block by block off one table, reading `@notes`, the sum of the terms
/// added. A change builds only what it changed or newly reads; the rest stays as it stood.
pub struct Stream {
    config: StreamConfig,
    world: World,
    driver: Driver,
    expr: Expr,
    terms: Terms,
    width: usize,
    supports: BTreeMap<Handle, Extent>,
    /// The first sounding term's end, which `supports` evicts.
    ending: Option<Option<i64>>,
    met: Lookups,
    generation: u64,
    live: bool,
    dropped: Recent<String>,
    late: usize,
    built: Built,
    demands: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub dropped: usize,
    /// Edits that landed past where issued.
    pub late: usize,
    pub terms: usize,
    /// The latest change's, or the open's.
    pub built: Built,
    /// The reads that worked out what the root asks of the note sum.
    pub demands: usize,
}

/// What one change did anew, not what it carried over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Built {
    /// Texts it took in: its expression and each node it newly reads.
    pub parsed: usize,
    pub instances: usize,
    /// Instances its walk to the store visited.
    pub visited: usize,
    pub typed: usize,
    pub values: usize,
    /// Values it made taking an old one's state.
    pub copied: usize,
    pub lookups: usize,
}

/// Each lookup met, until its store moves the samples.
#[derive(Default)]
struct Lookups {
    epoch: u64,
    known: Met,
}

impl Lookups {
    fn over(&mut self, store: &impl Through) -> &Met {
        if store.epoch() != self.epoch {
            self.known.clear();
            self.epoch = store.epoch();
        }
        &self.known
    }

    /// Kept where the store has not changed since `epoch`.
    fn noted(&mut self, key: Hash, found: &Option<Arc<Stored>>, epoch: u64, store: &impl Through) {
        self.over(store);
        if epoch == self.epoch {
            self.known.insert(key, found.clone());
        }
    }
}

impl Stream {
    pub async fn open(
        graph: &Graph,
        target: &Expr,
        config: StreamConfig,
        cache: Option<&Cache>,
        store: &impl Through,
    ) -> Result<Stream, EngineError> {
        let render = blocked(&config)?.render.clone();
        let world = World::new(graph, &render)?;
        let recording = Recording::over(cache, config.render.cache_policy).latest(LATEST);
        let table = Table::new(&render.profile);
        let driver = Driver::new(table, Extent::new(0, 0), config.block, &render, recording);
        let mut stream = Stream {
            world,
            driver,
            expr: target.clone(),
            terms: Terms::default(),
            width: 0,
            supports: BTreeMap::new(),
            ending: None,
            met: Lookups::default(),
            generation: 0,
            live: false,
            dropped: Recent::keeping(LATEST),
            late: 0,
            built: Built::default(),
            demands: 0,
            config,
        };
        let opening = Prospect {
            target: target.clone(),
            terms: Terms::default(),
            term: None,
            from: None,
            answer: Changed::Edited,
            landing: None,
            parsed: 1,
        };
        let mut local = Local::default();
        loop {
            match stream.attempt(&opening, &mut local)? {
                Attempt::Landed(_) => break,
                Attempt::Asks(keys) => {
                    let epoch = store.epoch();
                    local.look(keys, store).await;
                    for (key, found) in std::mem::take(&mut local.recorded) {
                        stream.met.noted(key, &found, epoch, store);
                    }
                }
                Attempt::Reads(wants) => local.read(wants, store).await,
                Attempt::Moved => unreachable!("nothing plays a stream before it opens"),
            }
        }
        let next = ahead(stream.driver.start, render.rate);
        through::load(&mut stream.driver.table, store, next, &BTreeSet::new()).await;
        Ok(stream)
    }

    pub fn graph(&self) -> &Graph {
        &self.world.graph
    }

    fn prospect(&self, change: Change) -> Result<Prospect, Changed> {
        let mut landing = None;
        let prospect = |terms, term: Option<(Handle, Expr)>, from: Option<Graph>, answer| {
            let roots = |(handle, term): &(Handle, Expr)| sva_ast::reads_of(&handle.node(), term);
            Prospect {
                target: self.expr.clone(),
                terms,
                from: from.map(|graph| (graph, term.as_ref().map(roots).unwrap_or_default())),
                term,
                answer,
                landing: None,
                parsed: 1,
            }
        };
        Ok(match change {
            Change::Target(graph, target) => {
                let roots = sva_ast::reads_of(STREAMED, &target);
                Prospect {
                    target,
                    from: Some((graph, roots)),
                    ..prospect(self.terms.clone(), None, None, Changed::Edited)
                }
            }
            Change::Add(graph, term, at) => {
                let term = match at {
                    Placed::Written => term,
                    Placed::Landing => placed(&term, *landing.insert(self.driver.at)),
                };
                let (terms, handle) = self.terms.added();
                Prospect {
                    landing,
                    ..prospect(
                        terms,
                        Some((handle, term)),
                        Some(graph),
                        Changed::Added(handle),
                    )
                }
            }
            Change::Replace(handle, graph, term, at) => {
                let landed = self.terms.landed(handle).ok_or(Changed::Held(false))?;
                let term = match at {
                    Placed::Written => term,
                    Placed::Landing => placed(&term, landed),
                };
                let terms = self.terms.clone();
                prospect(
                    terms,
                    Some((handle, term)),
                    Some(graph),
                    Changed::Held(true),
                )
            }
            Change::Remove(handle) => {
                let at = self.driver.at as f64 / f64::from(self.config.render.rate);
                let terms = self.terms.removed(handle).ok_or(Changed::Held(false))?;
                let held = self.world.graph.expr(&handle.node());
                let term = cut(held.expect("a held term's node"), at);
                Prospect {
                    parsed: 0,
                    ..prospect(terms, Some((handle, term)), None, Changed::Held(true))
                }
            }
        })
    }

    /// One try at `prospect`: planned, typed and built beside what plays, then held, or undone
    /// where the store must answer first.
    fn attempt(&mut self, prospect: &Prospect, local: &mut Local) -> Result<Attempt, EngineError> {
        if prospect.landing.is_some_and(|at| at != self.driver.at) {
            return Ok(Attempt::Moved);
        }
        let wanted = Wanted {
            target: &prospect.target,
            terms: &prospect.terms,
            term: prospect.term.as_ref().map(|(h, e)| (*h, e)),
            from: prospect.from.as_ref().map(|(g, roots)| (g, roots.clone())),
        };
        let met = &self.met.known;
        let found = |key: Hash| local.known.get(&key).or_else(|| met.get(&key)).cloned();
        let walked = self.world.plan(&wanted, (&found, &local.again))?;
        let mut plan = match walked {
            Walked::Asks(keys) => return Ok(Attempt::Asks(keys)),
            Walked::Planned(plan) => plan,
        };
        let built = self.built(prospect, &mut plan);
        let (root, range) = match built {
            Ok(held) => held,
            Err(e) => {
                let freed = self.world.abort();
                self.driver.table.abort(&freed);
                return Err(e);
            }
        };
        let window = ahead(self.driver.at.max(range.start), self.config.render.rate);
        let wants = self.driver.table.wants_made(root, window);
        let wants: Vec<(Arc<Stored>, Extent)> = wants
            .into_iter()
            .filter(|(stored, over)| !local.holds(stored.key, *over))
            .collect();
        if !wants.is_empty() {
            let freed = self.world.abort();
            self.driver.table.abort(&freed);
            return Ok(Attempt::Reads(wants));
        }
        Ok(Attempt::Landed(self.land(
            prospect,
            (plan, root, range),
            local,
        )))
    }

    /// The plan typed and built: the root's value and its range.
    fn built(
        &mut self,
        prospect: &Prospect,
        plan: &mut Plan,
    ) -> Result<(usize, Extent), EngineError> {
        let world = &mut self.world;
        let typing = &mut world.typing;
        typing.lower(&world.instances, &plan.groups, &BTreeMap::new())?;
        let id = typing
            .id(STREAMED)
            .ok_or_else(|| EngineError::UnknownNode(STREAMED.to_string()))?;
        prospect.terms.name(typing);
        let prefixes: BTreeMap<NodeId, Arc<Stored>> = plan
            .prefixes
            .iter()
            .filter_map(|(path, stored)| Some((typing.id(path)?, Arc::clone(stored))))
            .collect();
        let table = &mut self.driver.table;
        let root = table.grow(typing, id, &prefixes)?;
        let plays = table.values[root].width;
        let mut render = self.config.render.clone();
        match self.width {
            0 if self.config.channels == Some(0) => {
                return Err(refusal("a stream of no channels".to_string()));
            }
            0 => widens(plays, self.config.channels.unwrap_or(plays))?,
            width => {
                widens(plays, width)?;
                render.range.start = Some(self.driver.start);
            }
        }
        let profile = &self.config.render.profile;
        let support = Supports::over(typing, profile, Some(&table.supports)).of(id);
        let range = range_over((&render, STREAMED), support, Ends::Pulled)?;
        Ok((root, range))
    }

    /// The change held, each value it made carrying on what it continues.
    fn land(
        &mut self,
        prospect: &Prospect,
        (mut plan, root, range): (Plan, usize, Extent),
        local: &mut Local,
    ) -> Changed {
        let freed = self.world.commit(&mut plan);
        let now = self.driver.at;
        let carried = self.driver.table.settled(root, &freed, (now, self.live));
        for (key, samples) in &local.fetched {
            self.driver.table.took(*key, samples);
        }
        for at in &carried.silent {
            self.dropped
                .push(self.driver.table.values[*at].name.clone());
        }
        match self.width {
            0 => {
                let plays = self.driver.table.values[root].width;
                self.width = self.config.channels.unwrap_or(plays);
                (self.driver.start, self.driver.at) = (range.start, range.start);
            }
            _ => self.late += usize::from(self.driver.at > local.issued),
        }
        let last = self.last(range.end);
        self.driver.bound(last);
        self.driver.recording.found(std::mem::take(&mut plan.hits));
        self.expr = prospect.target.clone();
        self.terms = prospect.terms.clone();
        if let Changed::Added(handle) = prospect.answer {
            self.terms.land(handle, self.driver.at);
        }
        if let Some((handle, _)) = prospect.term {
            self.supported(handle);
        }
        self.built = Built {
            parsed: prospect.parsed + plan.adopted,
            instances: plan.named,
            visited: plan.visited,
            typed: self.world.typing.lowered().len(),
            values: self.driver.table.built,
            copied: carried.taken,
            lookups: local.lookups,
        };
        self.generation += 1;
        self.prune();
        prospect.answer
    }

    /// `handle`'s support, where it still sounds.
    fn supported(&mut self, handle: Handle) {
        self.supports.remove(&handle);
        self.ending = None;
        if !self.terms.handles().any(|h| h == handle) {
            return;
        }
        let typing = &self.world.typing;
        let Some(id) = typing.id(&handle.node()) else {
            return;
        };
        let profile = &self.config.render.profile;
        let support = Supports::over(typing, profile, Some(&self.driver.table.supports)).of(id);
        self.supports.insert(handle, support);
    }

    /// Loads the stored samples the next second reads; a block reading one not loaded has it
    /// computed, or, live, started silent where not ready.
    pub async fn fetch(&mut self, store: &impl Through) {
        let next = ahead(self.driver.at, self.config.render.rate);
        through::load(&mut self.driver.table, store, next, &BTreeSet::new()).await;
    }

    /// What `fetch` reads, for a caller reading it as the stream plays.
    pub fn wanted(&self) -> Vec<(Arc<Stored>, Extent)> {
        let next = ahead(self.driver.at, self.config.render.rate);
        self.driver.table.wants(next)
    }

    pub fn took(&mut self, key: Hash, samples: &[Buffer]) {
        self.driver.table.took(key, samples);
    }

    /// `n` samples from `at`, `None` past the end. An `at` behind is refused; one ahead skips
    /// there, computing through the span, or, live, as `go_live` says.
    pub fn read(&mut self, at: i64, n: usize) -> Result<Option<Block>, EngineError> {
        let now = self.driver.at;
        if at < now {
            return Err(refused(
                "engine.stream_behind",
                format!("sample {at} is before sample {now}, where the stream stands"),
                "read from the stream's position or later",
            ));
        }
        if n == 0 {
            return Err(refused(
                "engine.empty_read",
                format!("a read of no samples at sample {at}"),
                "read one sample or more",
            ));
        }
        match self.live {
            true if at > now => {
                for silenced in self.driver.skip(at)? {
                    self.dropped
                        .push(self.driver.table.values[silenced].name.clone());
                }
            }
            _ => {
                let block = self.config.block;
                while self.driver.at < at {
                    let step = block.min((at - self.driver.at) as usize);
                    if !self.driver.pulled(step)? {
                        return Ok(None);
                    }
                    self.prune();
                }
            }
        }
        let block = self.driver.read(n)?;
        self.prune();
        Ok(block.map(|b| b.widened(self.width)))
    }

    /// An edited node with no state, or one a read skips past, starts silent, never computing
    /// its past, named in `dropped`; a formula reads on exactly.
    pub fn go_live(&mut self) {
        self.live = true;
        let last = self.last(self.driver.last());
        self.driver.bound(last);
    }

    fn last(&self, range_end: i64) -> i64 {
        match self.live {
            true => self.config.render.range.end.unwrap_or(i64::MAX),
            false => range_end,
        }
    }

    /// The latest nodes a live edit started silent, of `counts().dropped`.
    pub fn dropped(&self) -> Vec<&str> {
        self.dropped.iter().map(String::as_str).collect()
    }

    pub fn counts(&self) -> Counts {
        Counts {
            dropped: self.dropped.made(),
            late: self.late,
            terms: self.terms.count(),
            built: self.built,
            demands: self.demands,
        }
    }

    /// Retires every term whose support ended by now and before the first sample of `notes`
    /// the root asks from now on; nothing is worked out until a support can have ended.
    fn prune(&mut self) {
        let now = self.driver.at;
        let supports = &self.supports;
        let ending = *self
            .ending
            .get_or_insert_with(|| supports.values().map(|s| s.end).min());
        if ending.is_none_or(|end| end > now) {
            return;
        }
        let (table, tys) = (&self.driver.table, &self.world.typing);
        let last = self.driver.last();
        let asked = match tys.id(NOTES).and_then(|notes| table.of(notes)) {
            Some(notes) if now < last => {
                self.demands += 1;
                let needs = table.demand(Extent::new(now, last));
                needs[notes].hold.iter().next().map(|asked| asked.start)
            }
            Some(_) => None,
            None => Some(i64::MIN),
        };
        let supports = &self.supports;
        let gone = |handle: Handle| {
            let support = supports.get(&handle);
            support.is_some_and(|s| s.end <= now && asked.is_none_or(|from| s.end <= from))
        };
        let support = |handle: Handle| supports.get(&handle).copied();
        let went = self.terms.retire(&gone, &support);
        if !went.is_empty() {
            self.generation += 1;
        }
        for handle in went {
            self.supports.remove(&handle);
            self.ending = None;
        }
    }

    /// Every segment of its own clock the stream computed of `node`'s value, in order.
    pub fn evaluated(&self, node: &str) -> Vec<sva_samples::Extent> {
        let table = &self.driver.table;
        self.world
            .typing
            .id(node)
            .and_then(|id| table.of(id))
            .map_or(Vec::new(), |at| table.values[at].evaluated.clone())
    }

    pub fn pruned(&self) -> sva_samples::Pruned {
        self.driver.table.pruned(&self.world.typing)
    }

    pub fn landed(&self, handle: Handle) -> Option<i64> {
        self.terms.landed(handle)
    }

    pub fn position(&self) -> i64 {
        self.driver.at
    }

    pub fn work(&self) -> Work {
        self.driver.work
    }

    pub fn stats(&self) -> CacheStats {
        self.driver.recording.stats()
    }

    /// The bytes its values hold, samples and state.
    pub fn held_bytes(&self) -> usize {
        self.driver.table.bytes()
    }

    pub fn end(&self) -> Option<i64> {
        self.driver.end()
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn config(&self) -> &StreamConfig {
        &self.config
    }
}

/// An edit, each with the graph that reaches it and all the stream plays.
pub enum Change {
    Target(Graph, Expr),
    Add(Graph, Expr, Placed),
    Replace(Handle, Graph, Expr, Placed),
    /// A sounding term is cut where the stream stands as the edit is built; what played stays.
    Remove(Handle),
}

/// Where a term's sample 0 sits: the stream's own, or the sample its add lands at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    Written,
    Landing,
}

/// A replace or remove answers whether the stream still held its handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changed {
    Edited,
    Added(Handle),
    Held(bool),
}

/// What a change wants played, and what it answers once it lands.
struct Prospect {
    target: Expr,
    terms: Terms,
    term: Option<(Handle, Expr)>,
    from: Option<(Graph, Vec<String>)>,
    answer: Changed,
    landing: Option<i64>,
    /// Its own expression, where it brought one.
    parsed: usize,
}

/// What one try at a change came to.
enum Attempt {
    Landed(Changed),
    Asks(Vec<Hash>),
    Reads(Vec<(Arc<Stored>, Extent)>),
    /// The stream left the sample it was placed at.
    Moved,
}

/// What one change has looked up and read, across its tries.
#[derive(Default)]
struct Local {
    known: Met,
    /// Keys asked again, a miss met before.
    again: BTreeSet<Hash>,
    recorded: Vec<(Hash, Option<Arc<Stored>>)>,
    fetched: Vec<(Hash, Vec<Buffer>)>,
    asked: Vec<(Hash, Extent)>,
    unread: BTreeSet<Hash>,
    lookups: usize,
    issued: i64,
}

impl Local {
    async fn look(&mut self, keys: Vec<Hash>, store: &impl Through) {
        for key in keys {
            self.lookups += 1;
            let found = store.lookup(key).await.map(Arc::new);
            self.again.insert(key);
            self.known.insert(key, found.clone());
            self.recorded.push((key, found));
        }
    }

    async fn read(&mut self, wants: Vec<(Arc<Stored>, Extent)>, store: &impl Through) {
        for (stored, over) in wants {
            let again = self
                .asked
                .iter()
                .any(|(key, e)| *key == stored.key && !e.intersect(over).is_empty());
            self.asked.push((stored.key, over));
            let read = match again {
                false => store.read(&stored, over).await,
                true => None,
            };
            match read {
                Some(samples) => self.fetched.push((stored.key, samples)),
                None => {
                    self.unread.insert(stored.key);
                }
            }
        }
    }

    /// Whether this change already read `key` over `over`, or could not.
    fn holds(&self, key: Hash, over: Extent) -> bool {
        self.unread.contains(&key)
            || self
                .asked
                .iter()
                .any(|(k, e)| *k == key && e.intersect(over) == over)
    }
}

/// `build`'s change, the stream held only while each try runs, between two blocks: an exact
/// stream pulled once its edit is done plays it where issued. `build` runs again whenever the
/// stream changed under it.
pub async fn change<E: From<EngineError>>(
    stream: &RefCell<Stream>,
    mut build: impl FnMut(&Stream) -> Result<Change, E>,
    store: &impl Through,
) -> Result<Changed, E> {
    let mut local = Local {
        issued: stream.borrow().driver.at,
        ..Local::default()
    };
    loop {
        let change = build(&stream.borrow())?;
        let prospect = match stream.borrow().prospect(change) {
            Ok(prospect) => prospect,
            Err(answer) => return Ok(answer),
        };
        let generation = stream.borrow().generation;
        loop {
            if stream.borrow().generation != generation {
                break;
            }
            stream.borrow_mut().met.over(store);
            let attempt = stream.borrow_mut().attempt(&prospect, &mut local)?;
            match attempt {
                Attempt::Landed(answer) => return Ok(answer),
                Attempt::Moved => break,
                Attempt::Asks(keys) => {
                    let epoch = store.epoch();
                    local.look(keys, store).await;
                    for (key, found) in std::mem::take(&mut local.recorded) {
                        stream.borrow_mut().met.noted(key, &found, epoch, store);
                    }
                }
                Attempt::Reads(wants) => local.read(wants, store).await,
            }
        }
    }
}

fn ahead(at: i64, rate: u32) -> Extent {
    Extent::new(at, at.saturating_add(i64::from(rate)))
}

fn widens(plays: usize, width: usize) -> Result<(), EngineError> {
    match plays == width || plays == 1 {
        true => Ok(()),
        false => Err(refused(
            "engine.stream_width",
            format!("this plays {plays} channel(s), and the stream plays {width}"),
            "play as many channels as the stream, or one, or open a new stream for it",
        )),
    }
}

fn blocked(config: &StreamConfig) -> Result<&StreamConfig, EngineError> {
    match config.block {
        0 => Err(refusal("a block of no samples".to_string())),
        _ => Ok(config),
    }
}

pub(super) fn refusal(what: String) -> EngineError {
    refused(
        "engine.no_stream",
        format!("this target opens no stream: {what}"),
        "stream an expression over the nodes the composition defines",
    )
}

fn refused(code: &str, message: String, help: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message,
        location: Located::at(STREAMED, None),
        help: help.to_string(),
    })
}
