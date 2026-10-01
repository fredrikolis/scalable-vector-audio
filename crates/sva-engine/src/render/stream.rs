// Concern: opens a target as a stream over a store, editing it and its terms live | Non-concern: pulling its blocks, what an edit carries on | IO: (Graph, target) -> Stream; (Change) -> Changed

use std::cell::{Ref, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_ast::{Expr, Graph};
use sva_formula::{Hash, NodeId};
use sva_samples::{Buffer, Extent};

use super::drive::{Block, Driver};
use super::frontier::{Frontier, Known};
use super::table::support::Supports;
use super::table::{Beside, Table, edit};
use super::terms::{Handle, NOTES, Terms, placed};
use super::{Ends, Render, RenderConfig, range_over, through};
use crate::cache::{Cache, CacheStats, Lookup, Outcome, Recording, Stored, Through};
use crate::error::{Diagnostic, EngineError, Located};
use crate::flops::Work;
use crate::instantiate;
use crate::recent::Recent;
use crate::schedule;
use crate::typing;

#[cfg(test)]
mod rebuilt;

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

/// A target rendered block by block off one table. The target may read `@notes`, the sum of
/// the terms added under handles. A node the store answers plays from its samples. A hit is
/// looked up once; a node reading the stream's own note sum never, as no store holds one.
pub struct Stream {
    config: StreamConfig,
    played: Played,
    driver: Driver,
    graph: Graph,
    expr: Expr,
    terms: Terms,
    width: usize,
    supports: BTreeMap<Handle, Extent>,
    met: Met,
    generation: u64,
    live: bool,
    dropped: Recent<String>,
    late: usize,
    built: Built,
}

/// What a stream plays, as its latest build left it: the typed composition, and each
/// instance's store key and what it reads.
struct Played {
    shell: Render,
    keys: BTreeMap<String, Hash>,
    reads: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub dropped: usize,
    /// Edits that landed past the sample they were issued at.
    pub late: usize,
    pub terms: usize,
    /// The latest change's, or the open's.
    pub built: Built,
}

/// What one change built anew, not what it carried over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Built {
    pub instances: usize,
    pub typed: usize,
    pub values: usize,
    pub lookups: usize,
}

/// Each lookup met, until its store moves the samples.
#[derive(Default)]
struct Met {
    epoch: u64,
    known: Known,
}

impl Met {
    fn over(&mut self, store: &impl Through) -> &Known {
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

    fn entries(&mut self, store: &impl Through) -> Vec<(Hash, Option<Arc<Stored>>)> {
        let entries = self.over(store).iter();
        entries.map(|(key, found)| (*key, found.clone())).collect()
    }
}

struct Shelled {
    played: Played,
    range: Extent,
    table: Table,
    hits: Vec<Lookup>,
    built: Built,
}

impl Stream {
    pub async fn open(
        graph: &Graph,
        target: &Expr,
        config: StreamConfig,
        cache: Option<&Cache>,
        store: &impl Through,
    ) -> Result<Stream, EngineError> {
        let terms = Terms::default();
        let (mut known, epoch) = (Known::new(), store.epoch());
        let render = &blocked(&config)?.render;
        let lookups = std::cell::Cell::new(0);
        let walk = async |found: &mut Frontier<'_>| {
            lookups.set(found.walked(&mut known, store).await);
        };
        let mut found = shelled(graph, target, &terms, render, (walk, || None)).await?;
        found.built.lookups = lookups.get();
        let width = config.channels.unwrap_or(found.width());
        if width == 0 {
            return Err(refusal("a stream of no channels".to_string()));
        }
        widens(found.width(), width)?;
        let mut met = Met::default();
        for (key, found) in known {
            met.noted(key, &found, epoch, store);
        }
        let next = ahead(found.range.start, render.rate);
        through::load(&mut found.table, store, next, &BTreeSet::new()).await;
        let mut recording = Recording::over(cache, config.render.cache_policy).latest(LATEST);
        recording.found(found.hits);
        let driver = Driver::new(
            found.table,
            found.range,
            config.block,
            &config.render,
            recording,
        );
        Ok(Stream {
            graph: graph.clone(),
            expr: target.clone(),
            terms,
            width,
            supports: BTreeMap::new(),
            met,
            generation: 0,
            live: false,
            dropped: Recent::keeping(LATEST),
            late: 0,
            built: found.built,
            driver,
            config,
            played: found.played,
        })
    }

    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        std::iter::once(&self.expr).chain(self.terms.exprs())
    }

    fn prospect(&self, change: Change) -> Result<Prospect, Changed> {
        let mut render = self.config.render.clone();
        render.range.start = Some(self.driver.start);
        let mut landing = None;
        let (graph, target, terms, answer) = match change {
            Change::Target(graph, target) => (graph, target, self.terms.clone(), Changed::Edited),
            Change::Add(graph, term, at) => {
                let term = match at {
                    Placed::Written => term,
                    Placed::Landing => placed(&term, *landing.insert(self.driver.at)),
                };
                let (terms, handle) = self.terms.added(term);
                (graph, self.expr.clone(), terms, Changed::Added(handle))
            }
            Change::Replace(handle, graph, term, at) => {
                let terms = self.terms.replaced(handle, |landed| match at {
                    Placed::Written => term,
                    Placed::Landing => placed(&term, landed),
                });
                let terms = terms.ok_or(Changed::Held(false))?;
                (graph, self.expr.clone(), terms, Changed::Held(true))
            }
            Change::Remove(handle) => {
                let at = self.driver.at as f64 / f64::from(self.played.shell.rate());
                let terms = self.terms.removed(handle, at);
                let terms = terms.ok_or(Changed::Held(false))?;
                (
                    self.graph.clone(),
                    self.expr.clone(),
                    terms,
                    Changed::Held(true),
                )
            }
        };
        Ok(Prospect {
            graph,
            target,
            terms,
            answer,
            render,
            generation: self.generation,
            landing,
        })
    }

    /// The stored samples `table` asks over the next second that it brings in and nothing
    /// holds; `None` where the stream changed since `generation` or left `landing`.
    fn wanting(
        &self,
        (generation, landing): (u64, Option<i64>),
        table: &Table,
        unread: &BTreeSet<Hash>,
    ) -> Option<Vec<(Arc<Stored>, Extent)>> {
        if generation != self.generation || landing.is_some_and(|at| at != self.driver.at) {
            return None;
        }
        let sounding = self.driver.table.stored_keys();
        let wants = table.wants(ahead(self.driver.at, self.config.render.rate));
        let brought = wants.into_iter();
        let brought =
            brought.filter(|(s, _)| !sounding.contains(&s.key) && !unread.contains(&s.key));
        Some(brought.collect())
    }

    /// Values of the same identity carry on; a changed stateful one takes its predecessor's
    /// state; the rest start now.
    fn apply(
        &mut self,
        (prospect, issued): (Prospect, i64),
        shelled: Shelled,
        fetched: &[(Hash, Vec<Buffer>)],
    ) {
        let Shelled {
            played,
            range,
            mut table,
            hits,
            built,
        } = shelled;
        self.built = built;
        let old = std::mem::replace(&mut self.driver.table, Table::empty());
        let dropped = edit::carried(&mut table, old, self.driver.at, self.live);
        for (key, samples) in fetched {
            table.took(*key, samples);
        }
        for at in dropped {
            self.dropped.push(table.values[at].name.clone());
        }
        self.late += usize::from(self.driver.at > issued);
        let last = self.last(range.end);
        self.driver.replace(table, last);
        self.driver.recording.found(hits);
        self.played = played;
        self.graph = prospect.graph;
        self.expr = prospect.target;
        self.terms = prospect.terms;
        if let Changed::Added(handle) = prospect.answer {
            self.terms.land(handle, self.driver.at);
        }
        let (tys, profile) = (&self.played.shell.tys, &self.played.shell.config.profile);
        let supports = Supports::over(tys, profile, Some(&self.driver.table.supports));
        let support = |handle: Handle| Some((handle, supports.of(tys.id(&handle.node())?)));
        self.supports = self.terms.handles().filter_map(support).collect();
        self.generation += 1;
        self.prune();
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

    /// `n` samples from sample `at`, cut where the stream ends; `None` from there on. An `at`
    /// behind the stream is refused; one past it skips there, computing through the span, or,
    /// live, as `go_live` says.
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

    /// An edited node with no state there, or one a read skips past, starts silent there,
    /// never computing its past, and is named in `dropped`; a formula reads on exactly. Silent,
    /// it plays on.
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
        }
    }

    /// Retires every term whose support, pruned as a render prunes it, ended by now and before
    /// the first sample of `notes` the root's window from now on asks, as its demand finds it.
    fn prune(&mut self) {
        let (table, tys) = (&self.driver.table, &self.played.shell.tys);
        let (now, last) = (self.driver.at, self.driver.last());
        let asked = match tys.id(NOTES).and_then(|notes| table.of(notes)) {
            Some(notes) if now < last => {
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
        if self.terms.prune(&gone, &support) {
            self.generation += 1;
        }
    }

    /// Every segment of its own clock the stream computed of `node`'s value, in order.
    pub fn evaluated(&self, node: &str) -> Vec<sva_samples::Extent> {
        let table = &self.driver.table;
        self.played
            .shell
            .tys
            .id(node)
            .and_then(|id| table.of(id))
            .map_or(Vec::new(), |at| table.values[at].evaluated.clone())
    }

    pub fn pruned(&self) -> sva_samples::Pruned {
        self.driver.table.pruned()
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

struct Prospect {
    graph: Graph,
    target: Expr,
    terms: Terms,
    answer: Changed,
    render: RenderConfig,
    generation: u64,
    landing: Option<i64>,
}

/// `build`'s change, holding the stream only to apply it, between two blocks, once the store
/// answered what it brings in: an exact stream pulled only once its edit is done plays it where
/// it was issued. `build` runs again whenever the stream changed under it.
pub async fn change<E: From<EngineError>>(
    stream: &RefCell<Stream>,
    mut build: impl FnMut(&Stream) -> Result<Change, E>,
    store: &impl Through,
) -> Result<Changed, E> {
    let mut local = Met::default();
    let mut fetched: Vec<(Hash, Vec<Buffer>)> = Vec::new();
    let (mut unread, mut asked) = (BTreeSet::new(), Vec::<(Hash, Extent)>::new());
    let issued = stream.borrow().driver.at;
    let lookups = std::cell::Cell::new(0);
    loop {
        let change = build(&stream.borrow())?;
        let prospect = match stream.borrow().prospect(change) {
            Ok(prospect) => prospect,
            Err(answer) => return Ok(answer),
        };
        for (key, found) in stream.borrow_mut().met.entries(store) {
            local.noted(key, &found, store.epoch(), store);
        }
        let walk = async |found: &mut Frontier<'_>| {
            while let Some(key) = found.walk(local.over(store)) {
                let epoch = store.epoch();
                lookups.set(lookups.get() + 1);
                let found = store.lookup(key).await.map(Arc::new);
                stream.borrow_mut().met.noted(key, &found, epoch, store);
                local.noted(key, &found, epoch, store);
            }
        };
        let (graph, target) = (&prospect.graph, &prospect.target);
        let config = &prospect.render;
        let prior = || Some(stream.borrow());
        let mut shelled = shelled(graph, target, &prospect.terms, config, (walk, prior)).await?;
        widens(shelled.width(), stream.borrow().width())?;
        let standing = (prospect.generation, prospect.landing);
        loop {
            for (key, samples) in &fetched {
                shelled.table.took(*key, samples);
            }
            let wants = stream.borrow().wanting(standing, &shelled.table, &unread);
            let Some(wants) = wants else {
                break;
            };
            if wants.is_empty() {
                let answer = prospect.answer;
                shelled.built.lookups = lookups.get();
                stream
                    .borrow_mut()
                    .apply((prospect, issued), shelled, &fetched);
                return Ok(answer);
            }
            for (stored, over) in wants {
                let again = asked
                    .iter()
                    .any(|(key, e)| *key == stored.key && !e.intersect(over).is_empty());
                asked.push((stored.key, over));
                let read = match again {
                    false => store.read(&stored, over).await,
                    true => None,
                };
                match read {
                    Some(samples) => fetched.push((stored.key, samples)),
                    None => {
                        unread.insert(stored.key);
                    }
                }
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

/// The graph with `streamed` defined as `target` and `notes` as `terms`. A composition's own
/// `notes` stands while there is no term.
fn wrapped(graph: &Graph, target: &Expr, terms: &Terms) -> Result<Graph, EngineError> {
    let mut wrapped = graph.clone();
    if !terms.is_empty() && graph.defines(NOTES) {
        return Err(EngineError::refused(Diagnostic {
            code: "engine.no_stream".to_string(),
            message: format!(
                "a term is added to `@{NOTES}`, and this composition defines its own `{NOTES}`"
            ),
            location: Located::at(NOTES, None),
            help: format!("rename the composition's `{NOTES}`, or play it without adding terms"),
        }));
    }
    let own = terms.is_empty() && graph.defines(NOTES);
    let sum = (!own).then(|| (NOTES.to_string(), terms.sum()));
    let defined = std::iter::once((STREAMED.to_string(), target.clone()));
    for (name, body) in defined.chain(sum).chain(terms.nodes()) {
        if !wrapped.define(&name, body) {
            return Err(refusal(format!(
                "this composition already has a node named `{name}`"
            )));
        }
    }
    Ok(wrapped)
}

/// The wrapped graph typed whole, each node the store answers standing on its samples.
async fn shelled<'s>(
    graph: &Graph,
    target: &Expr,
    terms: &Terms,
    config: &RenderConfig,
    (walk, prior): (
        impl AsyncFnOnce(&mut Frontier<'_>),
        impl Fn() -> Option<Ref<'s, Stream>>,
    ),
) -> Result<Shelled, EngineError> {
    let wrapped = wrapped(graph, target, terms)?;
    let instances = instantiate::instantiate(&wrapped, STREAMED, config.rate)?;
    let named = instances.paths().count();
    let root = instances.instance_of(STREAMED)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&root))?;
    let keys = through::keys(&wrapped, &instances, &order, config);
    let mut found = Frontier::from((&instances, &order), &keys, &root, (config, true));
    if !terms.is_empty() {
        found.unstored(reading(&order, &instances.instance_of(NOTES)?));
        found.unstored(terms.handles().map(Handle::node));
    }
    if let Some(prior) = prior() {
        found.asking(anew(
            (&prior.played.keys, &prior.played.reads),
            &keys,
            &order,
        ));
    }
    walk(&mut found).await;
    let within = order.within(&found.visited);
    let prior = prior();
    let (mut tys, carried) = match &prior {
        Some(prior) => {
            let (held, now) = (&prior.played.keys, &keys);
            let same = |path: &str| held.get(path) == now.get(path);
            let typing = &prior.played.shell.tys;
            let prior_typing = typing::Prior {
                typing,
                same: &same,
            };
            let (tys, carried) = typing::infer_beside(&instances, &within, &prior_typing)?;
            (tys, Some(carried))
        }
        None => (
            typing::infer_over(&instances, &within, &BTreeMap::new())?,
            None,
        ),
    };
    let id = tys
        .id(&root)
        .ok_or_else(|| EngineError::UnknownNode(root.clone()))?;
    terms.name(&mut tys);
    let prefixes: BTreeMap<NodeId, Arc<Stored>> = std::mem::take(&mut found.stored)
        .into_iter()
        .map(|(path, stored)| (tys.id(&path).expect("a walked node is typed"), stored))
        .collect();
    let schedule = schedule::plan(&tys, id, &[]);
    let shell = Render::shell(tys, id, config.clone(), schedule);
    let profile = &shell.config.profile;
    let beside = match (&prior, &carried) {
        (Some(prior), Some(carried)) => Beside::of(&prior.driver.table, &carried.nodes),
        _ => Beside::default(),
    };
    let table = Table::prefixed(&shell.tys, shell.root, profile, (&prefixes, beside))?;
    drop(prior);
    let support = Supports::over(&shell.tys, profile, Some(&table.supports)).of(shell.root);
    let range = range_over(&shell, support, Ends::Pulled)?;
    let hits = std::mem::take(&mut found.lookups).into_iter();
    let hits = hits.filter(|l| l.outcome == Outcome::Hit).collect();
    drop(found);
    let built = Built {
        instances: named,
        typed: shell.tys.lowered().len(),
        values: table.built,
        lookups: 0,
    };
    let reads = order.into_deps();
    Ok(Shelled {
        played: Played { shell, keys, reads },
        range,
        table,
        hits,
        built,
    })
}

/// The key of each read a changed node makes and did not in `prior`: any holder may fill a
/// miss, so one read anew is asked again.
fn anew(
    (prior, reads): (&BTreeMap<String, Hash>, &BTreeMap<String, Vec<String>>),
    keys: &BTreeMap<String, Hash>,
    order: &schedule::Order,
) -> BTreeSet<Hash> {
    let changed = keys
        .iter()
        .filter(|(path, key)| prior.get(*path) != Some(key));
    let mut out = BTreeSet::new();
    for (path, _) in changed {
        let before = reads.get(path).map_or(&[][..], Vec::as_slice);
        let new = order
            .deps(path)
            .iter()
            .filter(|read| !before.contains(read));
        out.extend(new.filter_map(|read| keys.get(read)));
    }
    out
}

impl Shelled {
    fn width(&self) -> usize {
        self.table.values[self.table.root].width
    }
}

/// `path` and every node reading it, however far down.
fn reading(order: &schedule::Order, path: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::from([path.to_string()]);
    loop {
        let more: Vec<String> = order
            .groups
            .iter()
            .flatten()
            .filter(|node| !out.contains(*node))
            .filter(|node| order.deps(node).iter().any(|read| out.contains(read)))
            .cloned()
            .collect();
        if more.is_empty() {
            return out;
        }
        out.extend(more);
    }
}

fn refusal(what: String) -> EngineError {
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
