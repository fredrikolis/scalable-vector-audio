// Concern: opens a target as a stream and edits it and its terms as it plays | Non-concern: pulling its blocks, what an edit carries on (edit.rs) | IO: (&Graph, target) -> Stream; (expr) -> Handle, bool

use sva_ast::{Expr, Graph};

use super::drive::{Block, Driver};
use super::table::{Table, edit};
use super::terms::{Handle, NOTES, Terms};
use super::{Ends, Render, RenderConfig, prepared, range_of};
use crate::cache::{Cache, CacheStats, Recording};
use crate::error::{Diagnostic, EngineError, Located};
use crate::flops::Work;
use crate::schedule;

pub const STREAMED: &str = "streamed";

#[derive(Clone, Debug, PartialEq)]
pub struct StreamConfig {
    pub block: usize,
    pub render: RenderConfig,
}

/// A target rendered block by block off one table. The target may read `@notes`, the sum of
/// the terms added under handles; an edit to it or to a term plays from the next block on.
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

impl Stream {
    pub fn open(
        graph: &Graph,
        target: &Expr,
        config: StreamConfig,
        cache: Option<&Cache>,
    ) -> Result<Stream, EngineError> {
        if config.block == 0 {
            return Err(refusal("a block of no samples".to_string()));
        }
        let mut terms = Terms::default();
        let (shell, range) = shelled(graph, target, &mut terms, &config.render)?;
        let table = Table::build(&shell.tys, shell.root, &[], &shell.config.profile)?;
        let recording = Recording::over(cache, config.render.cache_policy);
        let driver = Driver::new(table, range, config.block, &config.render, recording);
        Ok(Stream {
            graph: graph.clone(),
            expr: target.clone(),
            terms,
            live: false,
            dropped: Vec::new(),
            driver,
            config,
            shell,
        })
    }

    pub fn edit(&mut self, graph: &Graph, target: &Expr) -> Result<(), EngineError> {
        self.rebuilt(graph, target.clone(), self.terms.clone())
    }

    pub fn add(&mut self, graph: &Graph, term: &Expr) -> Result<Handle, EngineError> {
        let (terms, handle) = self.terms.added(term.clone());
        self.rebuilt(graph, self.expr.clone(), terms)?;
        Ok(handle)
    }

    /// False, and nothing edited, where the stream no longer holds `handle`.
    pub fn replace(
        &mut self,
        graph: &Graph,
        handle: Handle,
        term: &Expr,
    ) -> Result<bool, EngineError> {
        let Some(terms) = self.terms.replaced(handle, term.clone()) else {
            return Ok(false);
        };
        self.rebuilt(graph, self.expr.clone(), terms)?;
        Ok(true)
    }

    /// A sounding term is cut where the stream stands, so what it played stays what it was.
    pub fn remove(&mut self, handle: Handle) -> Result<bool, EngineError> {
        let at = self.driver.at as f64 / f64::from(self.shell.rate());
        let Some(terms) = self.terms.removed(handle, at) else {
            return Ok(false);
        };
        let graph = self.graph.clone();
        self.rebuilt(&graph, self.expr.clone(), terms)?;
        Ok(true)
    }

    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        std::iter::once(&self.expr).chain(self.terms.exprs())
    }

    /// Values of the same identity carry on; a changed stateful one takes its predecessor's
    /// state; the rest start now.
    fn rebuilt(
        &mut self,
        graph: &Graph,
        target: Expr,
        mut terms: Terms,
    ) -> Result<(), EngineError> {
        let mut render = self.config.render.clone();
        render.range.start = Some(self.driver.start);
        let (shell, range) = shelled(graph, &target, &mut terms, &render)?;
        let mut table = Table::build(&shell.tys, shell.root, &[], &shell.config.profile)?;
        let old = std::mem::replace(&mut self.driver.table, Table::empty());
        let dropped = edit::carried(&mut table, old, self.driver.at, self.live);
        self.dropped
            .extend(dropped.into_iter().map(|at| table.values[at].name.clone()));
        self.driver.replace(table, range.end);
        self.shell = shell;
        self.graph = graph.clone();
        self.expr = target;
        self.terms = terms;
        self.prune();
        Ok(())
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

/// The graph with `streamed` defined as `target` and `notes` as `terms`, typed, scheduled
/// and ranged. A composition's own `notes` stands while there is no term.
fn shelled(
    graph: &Graph,
    target: &Expr,
    terms: &mut Terms,
    config: &RenderConfig,
) -> Result<(Render, sva_samples::Extent), EngineError> {
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
    let mut held = prepared(&wrapped, STREAMED, config.rate)?;
    terms.typed(&held.instances, &mut held.tys);
    let schedule = schedule::plan(&held.tys, held.root, &[]);
    let shell = Render::shell(held.tys, held.root, config.clone(), schedule);
    let range = range_of(&shell, Ends::Pulled)?;
    Ok((shell, range))
}

fn refusal(what: String) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.no_stream".to_string(),
        message: format!("this target opens no stream: {what}"),
        location: Located::at(STREAMED, None),
        help: "stream an expression over the nodes the composition defines".to_string(),
    })
}
