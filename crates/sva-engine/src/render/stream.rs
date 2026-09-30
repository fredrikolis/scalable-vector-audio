// Concern: opens a target as a stream over a store, editing it and its terms live | Non-concern: pulling its blocks, what an edit carries on | IO: (Graph, target) -> Stream; (expr) -> Handle, bool

use std::collections::BTreeMap;
use std::sync::Arc;

use sva_ast::{Expr, Graph};
use sva_formula::NodeId;
use sva_samples::Extent;

use super::drive::{Block, Driver};
use super::frontier::Frontier;
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
/// the terms added under handles; an edit to it or to a term plays from the next block on.
/// Opening and each edit look `store` up first: a node it answers plays from its samples, its
/// own value computing what they miss.
pub struct Stream {
    config: StreamConfig,
    shell: Render,
    driver: Driver,
    graph: Graph,
    expr: Expr,
    terms: Terms,
    live: bool,
    dropped: Vec<String>,
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
        let render = &blocked(&config)?.render;
        let mut found = shelled(graph, target, &mut terms, render, store).await?;
        let next = ahead(found.range.start, render.rate);
        through::load(&mut found.table, store, next).await;
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
            live: false,
            dropped: Vec::new(),
            driver,
            config,
            shell: found.shell,
        })
    }

    pub async fn edit(
        &mut self,
        graph: &Graph,
        target: &Expr,
        store: &impl Through,
    ) -> Result<(), EngineError> {
        let terms = self.terms.clone();
        self.rebuilt(graph, target.clone(), terms, store).await
    }

    pub async fn add(
        &mut self,
        graph: &Graph,
        term: &Expr,
        store: &impl Through,
    ) -> Result<Handle, EngineError> {
        let (terms, handle) = self.terms.added(term.clone());
        self.rebuilt(graph, self.expr.clone(), terms, store).await?;
        Ok(handle)
    }

    /// False, and nothing edited, where the stream no longer holds `handle`.
    pub async fn replace(
        &mut self,
        graph: &Graph,
        (handle, term): (Handle, &Expr),
        store: &impl Through,
    ) -> Result<bool, EngineError> {
        let Some(terms) = self.terms.replaced(handle, term.clone()) else {
            return Ok(false);
        };
        self.rebuilt(graph, self.expr.clone(), terms, store).await?;
        Ok(true)
    }

    /// A sounding term is cut where the stream stands, so what it played stays what it was.
    pub async fn remove(
        &mut self,
        handle: Handle,
        store: &impl Through,
    ) -> Result<bool, EngineError> {
        let at = self.driver.at as f64 / f64::from(self.shell.rate());
        let Some(terms) = self.terms.removed(handle, at) else {
            return Ok(false);
        };
        let graph = self.graph.clone();
        self.rebuilt(&graph, self.expr.clone(), terms, store)
            .await?;
        Ok(true)
    }

    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        std::iter::once(&self.expr).chain(self.terms.exprs())
    }

    /// Values of the same identity carry on; a changed stateful one takes its predecessor's
    /// state; the rest start now.
    async fn rebuilt(
        &mut self,
        graph: &Graph,
        target: Expr,
        mut terms: Terms,
        store: &impl Through,
    ) -> Result<(), EngineError> {
        let mut render = self.config.render.clone();
        render.range.start = Some(self.driver.start);
        let found = shelled(graph, &target, &mut terms, &render, store).await?;
        let Shelled {
            shell,
            range,
            mut table,
            hits,
        } = found;
        let old = std::mem::replace(&mut self.driver.table, Table::empty());
        let dropped = edit::carried(&mut table, old, self.driver.at, self.live);
        through::load(&mut table, store, ahead(self.driver.at, render.rate)).await;
        self.dropped
            .extend(dropped.into_iter().map(|at| table.values[at].name.clone()));
        self.driver.replace(table, range.end);
        self.driver.recording.found(hits);
        self.shell = shell;
        self.graph = graph.clone();
        self.expr = target;
        self.terms = terms;
        self.prune();
        Ok(())
    }

    /// Loads the stored samples the next second reads; a block reading one not loaded has it
    /// computed, or, live, started silent where not ready.
    pub async fn fetch(&mut self, store: &impl Through) {
        let next = ahead(self.driver.at, self.config.render.rate);
        through::load(&mut self.driver.table, store, next).await;
    }

    /// What `fetch` reads, for a caller reading it as the stream plays.
    pub fn wanted(&self) -> Vec<(Arc<Stored>, Extent)> {
        let next = ahead(self.driver.at, self.config.render.rate);
        self.driver.table.wants(next)
    }

    pub fn took(&mut self, key: sva_formula::Hash, samples: Vec<sva_samples::Buffer>) {
        self.driver.table.took(key, samples);
    }

    /// The next block, cut where the stream ends; `None` from there on.
    pub fn next_block(&mut self) -> Result<Option<Block>, EngineError> {
        let block = self.driver.next_block()?;
        self.dropped.append(&mut self.driver.dropped);
        self.prune();
        Ok(block)
    }

    /// An edited node with no state there starts silent, never computing its past.
    pub fn go_live(&mut self) {
        self.live = true;
        self.driver.live = true;
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
        self.terms
            .prune(&gone, &|leaf| crate::refs::identity(&shell.tys, leaf).ok());
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

/// The wrapped graph typed whole, each node `store` answers standing on its samples.
async fn shelled(
    graph: &Graph,
    target: &Expr,
    terms: &mut Terms,
    config: &RenderConfig,
    store: &impl Through,
) -> Result<Shelled, EngineError> {
    let wrapped = wrapped(graph, target, terms)?;
    let instances = instantiate::instantiate(&wrapped, STREAMED, config.rate)?;
    let root = instances.instance_of(STREAMED)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&root))?;
    let keys = through::keys(&wrapped, &instances, &order, config);
    let mut found = Frontier::from((&instances, &order), &keys, &root, (config, true));
    found.walk(store).await;
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

fn refusal(what: String) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.no_stream".to_string(),
        message: format!("this target opens no stream: {what}"),
        location: Located::at(STREAMED, None),
        help: "stream an expression over the nodes the composition defines".to_string(),
    })
}
