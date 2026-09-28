// Concern: opens a target as a stream and edits its expression as it plays | Non-concern: pulling its blocks, what an edit carries on (edit.rs) | IO: (&Graph, target) -> a Stream; (&Graph, expr) -> ()

use std::collections::BTreeMap;

use sva_ast::{Expr, Graph};

use super::drive::node::{self, Driven, Hold, Kind};
use super::drive::{self, Block, Driver, edit};
use super::terms::Terms;
use super::{Lenses, Render, RenderConfig, prepared, reach, sampled};
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

/// A target rendered block by block, each node over the extent a whole render gives it; an
/// edit replaces the expression from the next block on.
pub struct Stream {
    config: StreamConfig,
    shell: Render,
    lenses: Lenses<'static>,
    driver: Driver,
    recording: Option<Recording>,
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
        let (shell, terms) = shelled(graph, target, &config.render)?;
        let range = shell.range.expect("audio out decides a range");
        let mut stream = Stream {
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
                &shell.config,
                true,
            ),
            recording: cache.map(|cache| Recording::over(cache, config.render.cache_policy)),
            config,
            lenses: Lenses::of(None, &shell),
            shell,
        };
        let lenses = stream.lenses.with(stream.recording.as_ref());
        let order = order(&stream.shell);
        let nodes = node::built(&stream.shell, &order, stream.hold(), &lenses, &|_| false)?;
        drop(lenses);
        let root = root_of(&stream.shell, &nodes)?;
        stream.driver.replace(nodes, Some(root), range.end);
        Ok(stream)
    }

    /// Unchanged nodes carry on; a changed one read where its predecessor was, at the same
    /// shift, takes its state where its kind, width and call sites do; the rest start now.
    pub fn edit(&mut self, graph: &Graph, target: &Expr) -> Result<(), EngineError> {
        let mut render = self.config.render.clone();
        render.range.start = Some(self.driver.start);
        let (shell, terms) = shelled(graph, target, &render)?;
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
        let hold = self.hold();
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
        self.expr = target.clone();
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

    /// What it plays, less each summand whose node ended bar a sum's last: the one to edit from.
    pub fn expression(&self) -> &Expr {
        &self.expr
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
        if let Some(pruned) = self.terms.pruned(&self.expr, &gone) {
            self.expr = pruned;
        }
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

    fn hold(&self) -> Hold {
        let root_keep = match self.config.render.until {
            Some(_) => drive::frame(&self.shell.config),
            None => 0,
        };
        Hold::Trailing {
            block: self.config.block,
            root_keep,
        }
    }
}

/// The graph with `streamed` defined as `target`, typed, scheduled and ranged.
fn shelled(
    graph: &Graph,
    target: &Expr,
    config: &RenderConfig,
) -> Result<(Render, Terms), EngineError> {
    let mut wrapped = graph.clone();
    if !wrapped.define(STREAMED, target.clone()) {
        return Err(refusal(format!(
            "this composition already has a node named `{STREAMED}`"
        )));
    }
    let held = prepared(&wrapped, STREAMED)?;
    let terms = Terms::of(
        &held.instances,
        &held.tys,
        &held.instances.instance_of(STREAMED)?,
    );
    sampled::on_the_grid(&held.tys, config.rate)?;
    let schedule = schedule::plan(&held.tys, &held.order, held.root, &[]);
    let audio = schedule.materialize.clone();
    let mut shell = Render::shell(held.tys, held.root, config.clone(), schedule);
    reach::streamed(&mut shell, &audio)?;
    Ok((shell, terms))
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
