// Concern: opens a target as a stream and edits its expression as it plays | Non-concern: pulling its blocks (drive/), what an edit carries on (edit.rs) | IO: (&Graph, target) -> a Stream; (expr) -> ()

use std::collections::BTreeMap;

use sva_ast::{Expr, Graph};

use super::drive::node::{self, Driven, Hold};
use super::drive::{self, Block, Driver, edit};
use super::{Render, RenderConfig, prepared, reach, sampled};
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
    driver: Driver,
}

impl Stream {
    pub fn open(graph: &Graph, target: &Expr, config: StreamConfig) -> Result<Stream, EngineError> {
        if config.block == 0 {
            return Err(refusal("a block of no samples".to_string()));
        }
        let shell = shelled(graph, target, &config.render)?;
        let range = shell.range.expect("audio out decides a range");
        let mut stream = Stream {
            driver: Driver::new(
                Vec::new(),
                None,
                range,
                config.block,
                config.render.until.clone(),
                &shell.config,
                true,
            ),
            config,
            shell,
        };
        let nodes = node::built(&stream.shell, &order(&stream.shell), stream.hold())?;
        let root = root_of(&stream.shell, &nodes)?;
        stream.driver.replace(nodes, Some(root), range.end);
        Ok(stream)
    }

    /// Unchanged nodes carry on; a changed one read where its predecessor was, at the same
    /// shift, takes its state where its kind, width and call sites do; the rest start now.
    pub fn edit(&mut self, graph: &Graph, target: &Expr) -> Result<(), EngineError> {
        let mut render = self.config.render.clone();
        render.range.start = Some(self.driver.start);
        let shell = shelled(graph, target, &render)?;
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
        let mut nodes = node::built(&shell, &order, self.hold())?;
        let root = root_of(&shell, &nodes)?;
        let old_root = self.driver.root;
        edit::carried(
            &mut nodes,
            &mut self.driver.nodes,
            &same,
            (Some(root), old_root),
            self.driver.at,
        );
        self.driver.replace(nodes, Some(root), range.end);
        self.shell = shell;
        Ok(())
    }

    /// The next block, cut where the stream ends; `None` from there on.
    pub fn next_block(&mut self) -> Result<Option<Block>, EngineError> {
        self.driver.next_block(&self.shell)
    }

    pub fn position(&self) -> i64 {
        self.driver.at
    }

    pub fn work(&self) -> Work {
        self.driver.work
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
fn shelled(graph: &Graph, target: &Expr, config: &RenderConfig) -> Result<Render, EngineError> {
    let mut wrapped = graph.clone();
    if !wrapped.define(STREAMED, target.clone()) {
        return Err(refusal(format!(
            "this composition already has a node named `{STREAMED}`"
        )));
    }
    let held = prepared(&wrapped, STREAMED)?;
    sampled::on_the_grid(&held.tys, config.rate)?;
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
