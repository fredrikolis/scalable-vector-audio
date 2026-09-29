// Concern: opens a target as a stream and edits it and its terms as it plays | Non-concern: pulling its blocks, what an edit carries on (edit.rs) | IO: (&Graph, target) -> Stream; (expr) -> Handle, bool

use std::collections::{BTreeMap, BTreeSet};

use sva_ast::{Expr, Graph};

use super::drive::node::{self, Driven, Hold, Kind};
use super::drive::{self, Block, Driver, edit};
use super::terms::{Handle, NOTES, Terms};
use super::{Lenses, Render, RenderConfig, prepared, reach};
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

/// A target rendered block by block, each node over the extent a whole render gives it. The
/// target may read `@notes`, the sum of the terms added under handles; an edit to it or to a
/// term plays from the next block on.
pub struct Stream {
    config: StreamConfig,
    shell: Render,
    lenses: Lenses<'static>,
    driver: Driver,
    recording: Option<Recording>,
    graph: Graph,
    expr: Expr,
    terms: Terms,
    ended: usize,
    live: bool,
    dropped: Vec<String>,
}

impl Stream {
    /// Reads and writes `cache` under its own policies, or the render's where it names one.
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
        let shell = shelled(graph, target, &mut terms, &config.render)?;
        let range = shell.range.expect("audio out decides a range");
        let mut stream = Stream {
            graph: graph.clone(),
            expr: target.clone(),
            terms,
            ended: 0,
            live: false,
            dropped: Vec::new(),
            driver: Driver::new(
                Vec::new(),
                None,
                range,
                config.block,
                config.render.until.clone(),
                &shell,
                BTreeSet::new(),
            ),
            recording: cache.map(|cache| Recording::over(cache, config.render.cache_policy)),
            config,
            lenses: Lenses::of(None, &shell),
            shell,
        };
        let lenses = stream.lenses.with(stream.recording.as_ref());
        let order = order(&stream.shell);
        let hold = stream.hold(&stream.shell);
        let nodes = node::built(&stream.shell, &order, hold, &lenses, &|_| false)?;
        drop(lenses);
        let root = root_of(&stream.shell, &nodes)?;
        stream.driver.replace(nodes, Some(root), range.end);
        Ok(stream)
    }

    /// `graph` reaches `target` and every term.
    pub fn edit(&mut self, graph: &Graph, target: &Expr) -> Result<(), EngineError> {
        self.rebuilt(graph, target.clone(), self.terms.clone())
    }

    /// `graph` reaches `term` and all the stream plays.
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
        let notes = self.shell.id(NOTES);
        let lag = self.driver.nodes.iter().find(|n| Some(n.id) == notes);
        let at = self.driver.at - lag.map_or(0, |n| n.lag);
        let at = at as f64 / f64::from(self.shell.rate());
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

    /// Unchanged nodes carry on; a changed one read where its predecessor was, at the same
    /// shift, takes its state where its kind, width and call sites do; the rest start now.
    fn rebuilt(
        &mut self,
        graph: &Graph,
        target: Expr,
        mut terms: Terms,
    ) -> Result<(), EngineError> {
        let mut render = self.config.render.clone();
        render.range.start = self.shell.range.map(|range| range.start);
        let shell = shelled(graph, &target, &mut terms, &render)?;
        let range = shell.range.expect("audio out decides a range");
        let order = order(&shell);
        let mut kept: BTreeMap<sva_formula::Hash, usize> = BTreeMap::new();
        for (at, node) in self.driver.nodes.iter().enumerate() {
            if let Ok(identity) = self.shell.identity(node.id) {
                kept.insert(identity, at);
            }
        }
        let mut same: Vec<Option<usize>> = Vec::with_capacity(order.len());
        for id in &order {
            let identity = shell.identity(*id).ok();
            same.push(identity.and_then(|identity| kept.remove(&identity)));
        }
        let hold = self.hold(&shell);
        let next = Lenses::of(None, &shell);
        let (was, now) = (
            self.lenses.with(self.recording.as_ref()),
            next.with(self.recording.as_ref()),
        );
        for (at, node) in self.driver.nodes.iter_mut().enumerate() {
            if !same.contains(&Some(at)) {
                node.store(&self.shell, &was);
            }
        }
        let mut nodes = node::built(&shell, &order, hold, &now, &|at| same[at].is_some())?;
        drop((was, now));
        let root = root_of(&shell, &nodes)?;
        let old_root = self.driver.root;
        let dropped = edit::carried(
            &mut nodes,
            &mut self.driver.nodes,
            &same,
            (Some(root), old_root),
            (self.driver.at, self.live),
        );
        self.dropped.extend(
            dropped
                .into_iter()
                .map(|at| shell.tys.name(nodes[at].id).to_string()),
        );
        self.driver.replace(nodes, Some(root), range.end);
        self.shell = shell;
        self.lenses = next;
        self.graph = graph.clone();
        self.expr = target;
        self.terms = terms;
        self.ended = 0;
        self.prune();
        Ok(())
    }

    /// The next block, cut where the stream ends; `None` from there on.
    pub fn next_block(&mut self) -> Result<Option<Block>, EngineError> {
        let lenses = self.lenses.with(self.recording.as_ref());
        let block = self.driver.next_block(&self.shell, &lenses)?;
        drop(lenses);
        self.prune();
        Ok(block)
    }

    /// An edit starts a changed node with no state at its instant silent there, never
    /// computing its past.
    pub fn go_live(&mut self) {
        self.live = true;
    }

    /// Each node a live edit started silent.
    pub fn dropped(&self) -> &[String] {
        &self.dropped
    }

    fn prune(&mut self) {
        let nodes = &self.driver.nodes;
        let ended = nodes
            .iter()
            .filter(|n| matches!(n.kind, Kind::Ended))
            .count();
        if ended == self.ended {
            return;
        }
        self.ended = ended;
        let mut read = vec![false; nodes.len()];
        read[self.driver.root.expect("a stream reads its root")] = true;
        for node in nodes {
            for &(at, _) in &node.reads {
                read[at] = true;
            }
        }
        let gone = |id| {
            nodes
                .iter()
                .zip(&read)
                .any(|(n, read)| *read && n.id == id && matches!(n.kind, Kind::Ended))
        };
        let shell = &self.shell;
        self.terms.prune(&gone, &|leaf| shell.identity(leaf).ok());
    }

    pub fn position(&self) -> i64 {
        self.driver.at
    }

    pub fn work(&self) -> Work {
        self.driver.work
    }

    /// Every lookup since it opened.
    pub fn stats(&self) -> CacheStats {
        self.recording
            .as_ref()
            .map(Recording::stats)
            .unwrap_or_default()
    }

    /// The bytes its nodes hold, samples and state.
    pub fn held_bytes(&self) -> usize {
        self.driver.bytes()
    }

    /// Where the stream ends, once known.
    pub fn end(&self) -> Option<i64> {
        self.driver.end()
    }

    pub fn width(&self) -> usize {
        self.driver.nodes[self.driver.root.expect("a stream reads its root")].width
    }

    pub fn config(&self) -> &StreamConfig {
        &self.config
    }

    /// What `shell`'s nodes keep behind the block they run: the root an `until` frame.
    fn hold(&self, shell: &Render) -> Hold {
        let root_keep = match self.config.render.until {
            Some(_) => drive::frame(&shell.config, shell.rate()),
            None => 0,
        };
        Hold::Trailing {
            block: self.config.block,
            root_keep,
        }
    }
}

/// The graph with `streamed` defined as `target` and `notes` as `terms`, typed, scheduled
/// and ranged. A composition's own `notes` stands while there is no term.
fn shelled(
    graph: &Graph,
    target: &Expr,
    terms: &mut Terms,
    config: &RenderConfig,
) -> Result<Render, EngineError> {
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
    let schedule = schedule::plan(&held.tys, &held.order, held.root, &[]);
    let audio = schedule.materialize.clone();
    let mut shell = Render::shell(held.tys, held.root, config.clone(), schedule);
    reach::streamed(&mut shell, &audio)?;
    Ok(shell)
}

fn order(shell: &Render) -> Vec<sva_formula::NodeId> {
    shell.schedule.materialize.clone()
}

fn root_of(shell: &Render, nodes: &[Driven]) -> Result<usize, EngineError> {
    nodes
        .iter()
        .position(|n| n.id == shell.root)
        .ok_or_else(|| EngineError::UnknownNode(shell.tys.name(shell.root).to_string()))
}

fn refusal(what: String) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.no_stream".to_string(),
        message: format!("this target opens no stream: {what}"),
        location: Located::at(STREAMED, None),
        help: "stream an expression over the nodes the composition defines".to_string(),
    })
}
