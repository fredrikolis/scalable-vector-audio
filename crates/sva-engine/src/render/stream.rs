// Concern: opens a target as a stream over a store, editing it and its terms live | Non-concern: pulling its blocks, what an edit carries on | IO: (Graph, target) -> Stream; (Change) -> Changed

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_ast::{Expr, Graph};
use sva_formula::{Hash, NodeId};
use sva_samples::{Buffer, Extent};

use super::drive::{Block, Driver};
use super::frontier::{Frontier, Known};
use super::table::{Table, edit};
use super::terms::{Handle, NOTES, Terms};
use super::{Ends, Render, RenderConfig, range_of, through};
use crate::cache::{Cache, CacheStats, Lookup, Outcome, Recording, Stored, Through};
use crate::error::{Diagnostic, EngineError, Located};
use crate::flops::Work;
use crate::instantiate;
use crate::schedule;
use crate::typing;

pub const STREAMED: &str = "streamed";

#[derive(Clone, Debug, PartialEq)]
pub struct StreamConfig {
    pub block: usize,
    pub render: RenderConfig,
}

/// A target rendered block by block off one table. The target may read `@notes`, the sum of
/// the terms added under handles. A node the store answers plays from its samples. A hit is
/// looked up once; a node reading the stream's own note sum never, as no store holds one.
pub struct Stream {
    config: StreamConfig,
    shell: Render,
    driver: Driver,
    graph: Graph,
    expr: Expr,
    terms: Terms,
    met: Met,
    generation: u64,
    live: bool,
    dropped: Vec<String>,
}

/// Each hit met: a header holds until its store moves the samples; any holder may fill a miss.
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

    fn hits(&mut self, store: &impl Through) -> Vec<(Hash, Option<Arc<Stored>>)> {
        let hits = self.over(store).iter().filter(|(_, found)| found.is_some());
        hits.map(|(key, found)| (*key, found.clone())).collect()
    }
}

struct Shelled {
    shell: Render,
    range: Extent,
    table: Table,
    hits: Vec<Lookup>,
}

impl Stream {
    pub async fn open(
        graph: &Graph,
        target: &Expr,
        config: StreamConfig,
        cache: Option<&Cache>,
        store: &impl Through,
    ) -> Result<Stream, EngineError> {
        let mut terms = Terms::default();
        let (mut known, epoch) = (Known::new(), store.epoch());
        let render = &blocked(&config)?.render;
        let walk = async |found: &mut Frontier<'_>| found.walked(&mut known, store).await;
        let mut found = shelled(graph, target, &mut terms, render, walk).await?;
        let mut met = Met::default();
        for (key, hit) in known.into_iter().filter(|(_, found)| found.is_some()) {
            met.noted(key, &hit, epoch, store);
        }
        let next = ahead(found.range.start, render.rate);
        through::load(&mut found.table, store, next, &BTreeSet::new()).await;
        let mut recording = Recording::over(cache, config.render.cache_policy);
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
            met,
            generation: 0,
            live: false,
            dropped: Vec::new(),
            driver,
            config,
            shell: found.shell,
        })
    }

    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        std::iter::once(&self.expr).chain(self.terms.exprs())
    }

    fn prospect(&self, change: Change) -> Result<Prospect, Changed> {
        let mut render = self.config.render.clone();
        render.range.start = Some(self.driver.start);
        let (graph, target, terms, answer) = match change {
            Change::Target(graph, target) => (graph, target, self.terms.clone(), Changed::Edited),
            Change::Add(graph, term) => {
                let (terms, handle) = self.terms.added(term);
                (graph, self.expr.clone(), terms, Changed::Added(handle))
            }
            Change::Replace(handle, graph, term) => {
                let terms = self.terms.replaced(handle, term);
                let terms = terms.ok_or(Changed::Held(false))?;
                (graph, self.expr.clone(), terms, Changed::Held(true))
            }
            Change::Remove(handle) => {
                let at = self.driver.at as f64 / f64::from(self.shell.rate());
                let terms = self.terms.removed(handle, at).ok_or(Changed::Held(false))?;
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
        })
    }

    /// The stored samples `table` asks over the next second that it brings in and nothing
    /// holds; `None` where the stream changed since `generation`.
    fn wanting(
        &self,
        generation: u64,
        table: &Table,
        unread: &BTreeSet<Hash>,
    ) -> Option<Vec<(Arc<Stored>, Extent)>> {
        if generation != self.generation {
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
    fn apply(&mut self, prospect: Prospect, shelled: Shelled, fetched: &[(Hash, Vec<Buffer>)]) {
        let Shelled {
            shell,
            range,
            mut table,
            hits,
        } = shelled;
        let old = std::mem::replace(&mut self.driver.table, Table::empty());
        let dropped = edit::carried(&mut table, old, self.driver.at, self.live);
        for (key, samples) in fetched {
            table.took(*key, samples);
        }
        self.dropped
            .extend(dropped.into_iter().map(|at| table.values[at].name.clone()));
        self.driver.replace(table, range.end);
        self.driver.recording.found(hits);
        self.shell = shell;
        self.graph = prospect.graph;
        self.expr = prospect.target;
        self.terms = prospect.terms;
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

    /// The next block, cut where the stream ends; `None` from there on.
    pub fn next_block(&mut self) -> Result<Option<Block>, EngineError> {
        let block = self.driver.next_block()?;
        self.prune();
        Ok(block)
    }

    /// An edited node with no state there starts silent, never computing its past.
    pub fn go_live(&mut self) {
        self.live = true;
    }

    /// Each node a live edit started silent.
    pub fn dropped(&self) -> &[String] {
        &self.dropped
    }

    /// Retires every term whose value no later block reads.
    fn prune(&mut self) {
        let (table, shell) = (&self.driver.table, &self.shell);
        let future = (self.driver.at < self.driver.last())
            .then(|| table.demand(sva_samples::Extent::new(self.driver.at, self.driver.last())));
        let gone = |id| {
            table.of(id).is_some_and(|at| {
                !table.values[at].evaluated.is_empty()
                    && future
                        .as_ref()
                        .is_none_or(|needs| needs[at].hold.is_empty())
            })
        };
        let named = |leaf| crate::refs::identity(&shell.tys, leaf).ok();
        if self.terms.prune(&gone, &named) {
            self.generation += 1;
        }
    }

    /// Every segment of its own clock the stream computed of `node`'s value, in order.
    pub fn evaluated(&self, node: &str) -> Vec<sva_samples::Extent> {
        let table = &self.driver.table;
        self.shell
            .tys
            .id(node)
            .and_then(|id| table.of(id))
            .map_or(Vec::new(), |at| table.values[at].evaluated.clone())
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
        self.driver.table.values[self.driver.table.root].width
    }

    pub fn config(&self) -> &StreamConfig {
        &self.config
    }
}

/// An edit, each with the graph that reaches it and all the stream plays.
pub enum Change {
    Target(Graph, Expr),
    Add(Graph, Expr),
    Replace(Handle, Graph, Expr),
    /// A sounding term is cut where the stream stands as the edit is built; what played stays.
    Remove(Handle),
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
    loop {
        let change = build(&stream.borrow())?;
        let mut prospect = match stream.borrow().prospect(change) {
            Ok(prospect) => prospect,
            Err(answer) => return Ok(answer),
        };
        for (key, hit) in stream.borrow_mut().met.hits(store) {
            local.noted(key, &hit, store.epoch(), store);
        }
        let walk = async |found: &mut Frontier<'_>| {
            while let Some(key) = found.walk(local.over(store)) {
                let epoch = store.epoch();
                let found = store.lookup(key).await.map(Arc::new);
                if found.is_some() {
                    stream.borrow_mut().met.noted(key, &found, epoch, store);
                }
                local.noted(key, &found, epoch, store);
            }
        };
        let (graph, target) = (&prospect.graph, &prospect.target);
        let config = &prospect.render;
        let mut shelled = shelled(graph, target, &mut prospect.terms, config, walk).await?;
        let generation = prospect.generation;
        loop {
            for (key, samples) in &fetched {
                shelled.table.took(*key, samples);
            }
            let wants = stream.borrow().wanting(generation, &shelled.table, &unread);
            let Some(wants) = wants else {
                break;
            };
            if wants.is_empty() {
                let answer = prospect.answer;
                stream.borrow_mut().apply(prospect, shelled, &fetched);
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
    let sum = (!own).then(|| (NOTES, terms.sum()));
    for (name, body) in std::iter::once((STREAMED, target.clone())).chain(sum) {
        if !wrapped.define(name, body) {
            return Err(refusal(format!(
                "this composition already has a node named `{name}`"
            )));
        }
    }
    Ok(wrapped)
}

/// The wrapped graph typed whole, each node the store answers standing on its samples.
async fn shelled(
    graph: &Graph,
    target: &Expr,
    terms: &mut Terms,
    config: &RenderConfig,
    walk: impl AsyncFnOnce(&mut Frontier<'_>),
) -> Result<Shelled, EngineError> {
    let wrapped = wrapped(graph, target, terms)?;
    let instances = instantiate::instantiate(&wrapped, STREAMED, config.rate)?;
    let root = instances.instance_of(STREAMED)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&root))?;
    let keys = through::keys(&wrapped, &instances, &order, config);
    let mut found = Frontier::from((&instances, &order), &keys, &root, (config, true));
    if !terms.is_empty() {
        found.unstored(reading(&order, &instances.instance_of(NOTES)?));
    }
    walk(&mut found).await;
    let mut tys = typing::infer_over(&instances, &order.within(&found.visited), &BTreeMap::new())?;
    let id = tys
        .id(&root)
        .ok_or_else(|| EngineError::UnknownNode(root.clone()))?;
    terms.typed(&instances, &mut tys);
    let prefixes: BTreeMap<NodeId, Arc<Stored>> = std::mem::take(&mut found.stored)
        .into_iter()
        .map(|(path, stored)| (tys.id(&path).expect("a walked node is typed"), stored))
        .collect();
    let schedule = schedule::plan(&tys, id, &[]);
    let shell = Render::shell(tys, id, config.clone(), schedule);
    let range = range_of(&shell, Ends::Pulled)?;
    let table = Table::prefixed(&shell.tys, shell.root, &shell.config.profile, &prefixes)?;
    let hits = found.lookups.into_iter();
    let hits = hits.filter(|l| l.outcome == Outcome::Hit).collect();
    Ok(Shelled {
        shell,
        range,
        table,
        hits,
    })
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
    EngineError::refused(Diagnostic {
        code: "engine.no_stream".to_string(),
        message: format!("this target opens no stream: {what}"),
        location: Located::at(STREAMED, None),
        help: "stream an expression over the nodes the composition defines".to_string(),
    })
}
