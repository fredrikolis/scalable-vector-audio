// Concern: renders a target block after block, each node carrying its state across blocks | Non-concern: one node's rows or machine, a whole-horizon render | IO: (&Graph, target, bindings) -> blocks

mod live;
mod node;

use std::cell::RefCell;
use std::collections::BTreeMap;

use sva_ast::Graph;
use sva_samples::Tape;
use sva_samples::physics::chaigne_askenfelt::landing_step;

use super::silent::{Silent, bound_from, not_silent_by};
use super::{Render, RenderConfig, prepared};
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::RELEASE;
use crate::schedule;
use node::Streamed;

pub const STREAMED: &str = "streamed";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamConfig {
    pub rate: u32,
    pub block: usize,
    /// Ends the stream where the tail bound proves silence; `None` streams on forever.
    pub silent: Option<Silent>,
}

/// A target rendered from the grid's first sample on, one block at a time: every block is the
/// samples a whole render over the same rows writes there, bit for bit.
pub struct Stream {
    graph: Graph,
    target: String,
    bindings: Vec<(String, f64)>,
    config: StreamConfig,
    shell: Render,
    nodes: Vec<Streamed>,
    root: usize,
    at: usize,
    /// Silence at the threshold, to be proven by `limit`, and the last bound found.
    silent: Option<(Silent, usize)>,
    bound: f64,
    heard: RefCell<BTreeMap<(sva_formula::NodeId, u64), sva_samples::Buffer>>,
    end: Option<usize>,
}

/// The samples one `next` wrote, `[start, start + len)` of the grid.
pub struct Block<'a> {
    tape: &'a Tape,
    start: usize,
}

impl Block<'_> {
    pub fn start(&self) -> usize {
        self.start
    }

    pub fn len(&self) -> usize {
        self.tape.end() - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn width(&self) -> usize {
        self.tape.width()
    }

    pub fn plane(&self, c: usize) -> &[f64] {
        self.tape.since(c, self.start)
    }
}

impl Stream {
    /// `bindings` are the named arguments `@target(t, name=value, ...)` is written with.
    pub fn open(
        graph: &Graph,
        target: &str,
        bindings: &[(String, f64)],
        config: StreamConfig,
    ) -> Result<Stream, EngineError> {
        let mut stream = Stream::opened_at(graph, target, bindings, config, 0)?;
        stream.settle()?;
        Ok(stream)
    }

    /// Silence is proven by `max_secs` past `from`.
    fn opened_at(
        graph: &Graph,
        target: &str,
        bindings: &[(String, f64)],
        config: StreamConfig,
        from: usize,
    ) -> Result<Stream, EngineError> {
        if config.block == 0 {
            return Err(refusal(target, "a block of no samples".to_string()));
        }
        let wrapped = bound(graph, target, bindings)?;
        let held = prepared(&wrapped, STREAMED)?;
        let rate = f64::from(config.rate);
        let silent = config
            .silent
            .map(|silent| (silent, from + (silent.max_secs * rate).ceil() as usize));
        // No row a stream takes reads a horizon.
        let render_config = RenderConfig::seconds(config.rate, 1.0);
        let schedule = schedule::plan(&held.tys, &held.order, held.root, &[]);
        let shell = Render {
            root: held.root,
            tys: held.tys,
            buffers: BTreeMap::new(),
            frames: BTreeMap::new(),
            symbolic: BTreeMap::new(),
            labels: BTreeMap::new(),
            traces: Vec::new(),
            config: render_config,
            schedule,
            bindings: BTreeMap::new(),
            cache_stats: None,
        };
        let nodes = node::built(&shell, config.block)?;
        let root = nodes
            .iter()
            .position(|n| n.id == shell.root)
            .ok_or_else(|| EngineError::UnknownNode(shell.tys.name(shell.root).to_string()))?;
        Ok(Stream {
            graph: graph.clone(),
            target: target.to_string(),
            bindings: bindings.to_vec(),
            config,
            shell,
            nodes,
            root,
            at: 0,
            silent,
            bound: f64::INFINITY,
            heard: RefCell::new(BTreeMap::new()),
            end: None,
        })
    }

    /// Ends the stream here where every later sample is proven under the threshold from the
    /// states it holds now.
    fn settle(&mut self) -> Result<(), EngineError> {
        let (Some((silent, _)), None) = (self.silent, self.end) else {
            return Ok(());
        };
        let live = live::View::of(&self.nodes, self.at);
        let (tys, root) = (&self.shell.tys, self.shell.root);
        let config = &self.shell.config;
        self.bound = bound_from(tys, root, config, silent, &live, self.at, &self.heard)?;
        if self.bound < silent.threshold() {
            self.end = Some(self.at.max(1));
        }
        Ok(())
    }

    /// No block past the limit is taken while silence is unproven.
    fn in_time(&self) -> Result<(), EngineError> {
        match (self.silent, self.end) {
            (Some((silent, limit)), None) if self.at >= limit => {
                let max_secs = limit as f64 / f64::from(self.config.rate);
                let (tys, root) = (&self.shell.tys, self.shell.root);
                Err(not_silent_by(
                    tys,
                    root,
                    self.bound,
                    Silent { max_secs, ..silent },
                ))
            }
            _ => Ok(()),
        }
    }

    /// The next block, cut where silence was proven; `None` from there on.
    pub fn next_block(&mut self) -> Result<Option<Block<'_>>, EngineError> {
        self.in_time()?;
        let from = self.at;
        let to = match self.end {
            Some(end) if from >= end => return Ok(None),
            Some(end) => end.min(from + self.config.block),
            None => from + self.config.block,
        };
        for at in 0..self.nodes.len() {
            let (done, rest) = self.nodes.split_at_mut(at);
            rest[0].run(&self.shell, done, from, to)?;
        }
        self.at = to;
        self.settle()?;
        Ok(Some(Block {
            tape: &self.nodes[self.root].tape,
            start: from,
        }))
    }

    pub fn position(&self) -> usize {
        self.at
    }

    /// Where silence ends the stream, once a block has proven it.
    pub fn end(&self) -> Option<usize> {
        self.end
    }

    pub fn width(&self) -> usize {
        self.nodes[self.root].width
    }

    pub fn config(&self) -> StreamConfig {
        self.config
    }

    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            at: self.at,
            target: self.target.clone(),
            bindings: self.bindings.clone(),
            rate: self.config.rate,
            block: self.config.block,
            nodes: self.nodes.iter().map(Streamed::held).collect(),
        }
    }

    /// This stream's target from `checkpoint` on, `bindings` in force, ending at `silent`
    /// proven by `max_secs` past the checkpoint. A binding may move only where no sample
    /// before the checkpoint can hear it: `release`, at or past it.
    pub fn resume(
        &self,
        checkpoint: &Checkpoint,
        bindings: &[(String, f64)],
        silent: Option<Silent>,
    ) -> Result<Stream, EngineError> {
        let (rate, block) = (self.config.rate, self.config.block);
        if checkpoint.target != self.target || (checkpoint.rate, checkpoint.block) != (rate, block)
        {
            return Err(mismatch(&self.target, "another stream"));
        }
        causal(checkpoint, bindings, rate, &self.target)?;
        let config = StreamConfig {
            rate,
            block,
            silent,
        };
        let mut resumed =
            Stream::opened_at(&self.graph, &self.target, bindings, config, checkpoint.at)?;
        if resumed.nodes.len() != checkpoint.nodes.len() {
            return Err(mismatch(&self.target, "a graph of another shape"));
        }
        for (node, held) in resumed.nodes.iter_mut().zip(&checkpoint.nodes) {
            node.resume(&resumed.shell, held, checkpoint.at)?;
        }
        resumed.at = checkpoint.at;
        resumed.settle()?;
        Ok(resumed)
    }
}

/// Every node's state at a block's end, and what its stream was opened with.
#[derive(Clone)]
pub struct Checkpoint {
    at: usize,
    target: String,
    bindings: Vec<(String, f64)>,
    rate: u32,
    block: usize,
    nodes: Vec<Option<node::NodeState>>,
}

impl Checkpoint {
    pub fn position(&self) -> usize {
        self.at
    }
}

/// A moved `release` changes nothing before the step a felt released then lands on.
fn causal(
    checkpoint: &Checkpoint,
    bindings: &[(String, f64)],
    rate: u32,
    target: &str,
) -> Result<(), EngineError> {
    let value =
        |set: &[(String, f64)], name: &str| set.iter().find(|(n, _)| n == name).map(|(_, v)| *v);
    let names = checkpoint.bindings.iter().chain(bindings).map(|(n, _)| n);
    for name in names {
        let (was, now) = (value(&checkpoint.bindings, name), value(bindings, name));
        if was == now {
            continue;
        }
        let lands = |v: Option<f64>| {
            landing_step(v.unwrap_or(f64::INFINITY), f64::from(rate))
                .is_none_or(|at| at >= checkpoint.at as u64)
        };
        if name != RELEASE || !lands(was) || !lands(now) {
            return Err(EngineError::refused(Diagnostic {
                code: "engine.binding_not_causal".to_string(),
                message: format!(
                    "`{name}` moves from {} to {} at sample {}, and a sample before it can \
                     hear that",
                    shown(was),
                    shown(now),
                    checkpoint.at
                ),
                location: Located::at(target, None),
                help: "move only release, to a key-up at or past the checkpoint".to_string(),
            }));
        }
    }
    Ok(())
}

fn shown(value: Option<f64>) -> String {
    value.map_or("unbound".to_string(), |v| v.to_string())
}

fn mismatch(target: &str, what: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.checkpoint_mismatch".to_string(),
        message: format!("this checkpoint was taken of {what}, not of this `{target}` stream"),
        location: Located::at(target, None),
        help: "resume a checkpoint on the stream it was taken of".to_string(),
    })
}

/// The graph with `streamed` defined as the target called with `bindings`.
fn bound(graph: &Graph, target: &str, bindings: &[(String, f64)]) -> Result<Graph, EngineError> {
    if !graph.defines(target) {
        return Err(EngineError::UnknownNode(target.to_string()));
    }
    let mut named = String::new();
    for (name, value) in bindings {
        let word = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !word || !value.is_finite() {
            return Err(refusal(target, format!("`{name}` bound to {value}")));
        }
        named.push_str(&format!(", {name}={value}"));
    }
    let call = sva_ast::parse_expr(&format!("@{target}(t{named})"))
        .map_err(|d| refusal(target, d.message))?;
    let mut wrapped = graph.clone();
    if !wrapped.define(STREAMED, call) {
        return Err(refusal(
            target,
            format!("this composition already has a node named `{STREAMED}`"),
        ));
    }
    Ok(wrapped)
}

fn refusal(target: &str, what: String) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.no_stream".to_string(),
        message: format!("`{target}` opens no stream: {what}"),
        location: Located::at(target, None),
        help: "name a node the composition defines, and bind each name to a finite number"
            .to_string(),
    })
}
