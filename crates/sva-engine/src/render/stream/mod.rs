// Concern: renders a target block after block, each node carrying its state across blocks | Non-concern: one node's rows or machine, a whole render | IO: (&Graph, target) -> blocks

mod node;

use sva_ast::{Expr, Graph, Literal};
use sva_samples::physics::chaigne_askenfelt::landing_step;
use sva_samples::{Extent, Tape};

use super::until::Known;
use super::{Render, RenderConfig, prepared, reach, sampled};
use crate::error::{Diagnostic, EngineError, Located};
use crate::flops::Work;
use crate::instantiate::RELEASE;
use crate::query::DEFAULT_FRAME_SECS;
use crate::schedule;
use node::Streamed;

pub const STREAMED: &str = "streamed";

#[derive(Clone, Debug, PartialEq)]
pub struct StreamConfig {
    pub block: usize,
    pub render: RenderConfig,
}

/// A target rendered from its range's start on, one block at a time, each node over the same
/// extent a whole render gives it: every block is the samples such a render writes there.
pub struct Stream {
    graph: Graph,
    target: Expr,
    bindings: Vec<(String, f64)>,
    config: StreamConfig,
    shell: Render,
    nodes: Vec<Streamed>,
    root: usize,
    start: i64,
    at: i64,
    /// The range's end, or where the root, cut, ends where the range states none.
    last: i64,
    frame: usize,
    end: Option<i64>,
    work: Work,
}

/// The samples one `next` wrote, from grid sample `start` on.
pub struct Block {
    planes: Vec<Vec<f64>>,
    start: i64,
}

impl Block {
    /// `[start, end)` of the root, silent outside its support.
    fn of(tape: &Tape, support: Extent, start: i64, end: i64) -> Block {
        let window = tape.within(support);
        let planes = (0..tape.width())
            .map(|c| (start..end).map(|n| window.at(c, n)).collect())
            .collect();
        Block { planes, start }
    }

    pub fn start(&self) -> i64 {
        self.start
    }

    pub fn len(&self) -> usize {
        self.planes[0].len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn width(&self) -> usize {
        self.planes.len()
    }

    pub fn plane(&self, c: usize) -> &[f64] {
        &self.planes[c]
    }
}

impl Stream {
    /// Each named argument of `target`'s own ref written as a number is a binding a resume
    /// may move.
    pub fn open(graph: &Graph, target: &Expr, config: StreamConfig) -> Result<Stream, EngineError> {
        let (bare, bindings) = bindings_of(target);
        Stream::bound_to(graph, &bare, &bindings, config)
    }

    fn bound_to(
        graph: &Graph,
        target: &Expr,
        bindings: &[(String, f64)],
        config: StreamConfig,
    ) -> Result<Stream, EngineError> {
        if config.block == 0 {
            return Err(refusal("a block of no samples".to_string()));
        }
        let wrapped = bound(graph, target, bindings)?;
        let held = prepared(&wrapped, STREAMED)?;
        sampled::on_the_grid(&held.tys, config.render.rate)?;
        let schedule = schedule::plan(&held.tys, &held.order, held.root, &[]);
        let audio = schedule.materialize.clone();
        let mut shell = Render::shell(held.tys, held.root, config.render.clone(), schedule);
        reach::ranged(&mut shell, &audio, &audio)?;
        let range = shell.range.expect("audio out decides a range");
        let rate = config.render.rate;
        let frame = ((DEFAULT_FRAME_SECS * f64::from(rate)).round() as usize).max(1);
        let keep = match config.render.until {
            Some(_) => frame,
            None => 0,
        };
        let nodes = node::built(&shell, config.block, keep)?;
        let root = nodes
            .iter()
            .position(|n| n.id == shell.root)
            .ok_or_else(|| EngineError::UnknownNode(shell.tys.name(shell.root).to_string()))?;
        let work = Work {
            proofs: shell.proofs,
            waves: Some(0),
            ..Work::default()
        };
        Ok(Stream {
            graph: graph.clone(),
            target: target.clone(),
            bindings: bindings.to_vec(),
            config,
            shell,
            nodes,
            root,
            start: range.start,
            at: range.start,
            last: range.end,
            frame,
            end: None,
            work,
        })
    }

    /// A level is known once its frame is whole, so it stops the stream no earlier than the
    /// block it becomes known in.
    fn settle(&mut self, from: i64, to: i64) {
        if self.end.is_some() {
            return;
        }
        if self.last == to {
            self.end = Some(to);
        }
        let Some(until) = &self.config.render.until else {
            return;
        };
        let root = &self.nodes[self.root];
        let base = root.tape.base().max(self.start);
        let heard = Block::of(&root.tape, root.support, base, to);
        let rate = self.config.render.rate;
        let known = Known::new(
            heard.plane(0),
            base,
            self.start,
            self.frame,
            rate,
            to == self.last,
        );
        if let Some(at) = until.first(&known, base, to) {
            self.end = Some(at.max(from));
        }
    }

    /// The next block, cut where the stream ends; `None` from there on.
    pub fn next_block(&mut self) -> Result<Option<Block>, EngineError> {
        let from = self.at;
        if self.end.is_some_and(|end| from >= end) {
            return Ok(None);
        }
        let to = self.last.min(from + self.config.block as i64);
        for at in 0..self.nodes.len() {
            let (done, rest) = self.nodes.split_at_mut(at);
            rest[0].run(&self.shell, done, from, to)?;
            let (priced, waves) = rest[0].work(from, to);
            self.work.priced_flops += priced;
            self.work.waves = self.work.waves.zip(waves).map(|(held, more)| held + more);
        }
        self.at = to;
        self.work.samples += (to - from) as u64;
        self.settle(from, to);
        let end = self.end.map_or(to, |end| end.clamp(from, to));
        let root = &self.nodes[self.root];
        Ok(Some(Block::of(&root.tape, root.support, from, end)))
    }

    pub fn position(&self) -> i64 {
        self.at
    }

    /// Since this stream opened, or since the checkpoint it resumed from.
    pub fn work(&self) -> Work {
        self.work
    }

    /// Where the stream ends, once known.
    pub fn end(&self) -> Option<i64> {
        self.end
    }

    pub fn width(&self) -> usize {
        self.nodes[self.root].width
    }

    pub fn config(&self) -> &StreamConfig {
        &self.config
    }

    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            at: self.at,
            target: sva_ast::render_expr(&self.target),
            bindings: self.bindings.clone(),
            rate: self.config.render.rate,
            block: self.config.block,
            nodes: self.nodes.iter().map(Streamed::held).collect(),
        }
    }

    /// This stream's target from `checkpoint` on, each of `bindings` in force over the one
    /// it replaces. A binding may move only where no sample before the checkpoint can hear
    /// it: `release`, at or past it.
    pub fn resume(
        &self,
        checkpoint: &Checkpoint,
        bindings: &[(String, f64)],
    ) -> Result<Stream, EngineError> {
        let (rate, block) = (self.config.render.rate, self.config.block);
        let target = sva_ast::render_expr(&self.target);
        if checkpoint.target != target || (checkpoint.rate, checkpoint.block) != (rate, block) {
            return Err(mismatch(&target, "another stream"));
        }
        let mut merged = checkpoint.bindings.clone();
        for (name, value) in bindings {
            match merged.iter_mut().find(|(held, _)| held == name) {
                Some(slot) => slot.1 = *value,
                None => merged.push((name.clone(), *value)),
            }
        }
        words(bindings)?;
        causal(checkpoint, &merged, rate, &target)?;
        let mut resumed =
            Stream::bound_to(&self.graph, &self.target, &merged, self.config.clone())?;
        if resumed.nodes.len() != checkpoint.nodes.len() {
            return Err(mismatch(&target, "a graph of another shape"));
        }
        for (node, held) in resumed.nodes.iter_mut().zip(&checkpoint.nodes) {
            node.resume(&resumed.shell, held, checkpoint.at)?;
        }
        resumed.at = checkpoint.at;
        Ok(resumed)
    }
}

/// The target with every named argument its own ref writes as a number taken out, and those.
fn bindings_of(target: &Expr) -> (Expr, Vec<(String, f64)>) {
    let mut bare = target.clone();
    let mut out = Vec::new();
    if let Expr::Ref { binds, .. } = &mut bare {
        binds.retain(|(name, value)| match value {
            Expr::Lit(Literal::Num(v)) => {
                out.push((name.clone(), *v));
                false
            }
            _ => true,
        });
    }
    (bare, out)
}

/// Every node's state at a block's end, and what its stream was opened with.
#[derive(Clone)]
pub struct Checkpoint {
    at: i64,
    target: String,
    bindings: Vec<(String, f64)>,
    rate: u32,
    block: usize,
    nodes: Vec<Option<node::NodeState>>,
}

impl Checkpoint {
    pub fn position(&self) -> i64 {
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
                .is_none_or(|at| at as i64 >= checkpoint.at)
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

/// Each binding names a word and holds a finite number.
fn words(bindings: &[(String, f64)]) -> Result<(), EngineError> {
    for (name, value) in bindings {
        let word = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !word || !value.is_finite() {
            return Err(refusal(format!("`{name}` bound to {value}")));
        }
    }
    Ok(())
}

/// The graph with `streamed` defined as `target`, each of `bindings` a named argument on
/// its own ref.
fn bound(graph: &Graph, target: &Expr, bindings: &[(String, f64)]) -> Result<Graph, EngineError> {
    words(bindings)?;
    let mut call = target.clone();
    for (name, value) in bindings {
        let Expr::Ref { binds, .. } = &mut call else {
            return Err(refusal(format!(
                "`{name}` binds a ref, and `{}` is no ref",
                sva_ast::render_expr(target)
            )));
        };
        binds.push((name.clone(), Expr::Lit(sva_ast::Literal::Num(*value))));
    }
    let mut wrapped = graph.clone();
    if !wrapped.define(STREAMED, call) {
        return Err(refusal(format!(
            "this composition already has a node named `{STREAMED}`"
        )));
    }
    Ok(wrapped)
}

fn refusal(what: String) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.no_stream".to_string(),
        message: format!("this target opens no stream: {what}"),
        location: Located::at(STREAMED, None),
        help: "stream a ref the composition defines, and bind each name to a finite number"
            .to_string(),
    })
}
