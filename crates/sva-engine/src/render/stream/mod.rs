// Concern: renders a target block after block, each node carrying its state across blocks | Non-concern: one node's rows or machine, a whole render | IO: (&Graph, target) -> blocks

mod node;

use std::borrow::Cow;
use std::rc::Rc;

use sva_ast::{Expr, Graph, Literal};
use sva_formula::NodeId;
use sva_samples::physics::chaigne_askenfelt::landing_step;
use sva_samples::{Extent, Params, Tape, Walk};

use super::bound::{Forms, Played, STEP};
use super::cut::{Search, Sites, Span};
use super::extent::{self, Supports};
use super::until::Known;
use super::{Render, RenderConfig, prepared, reach, sampled};
use crate::error::{Diagnostic, EngineError, Located};
use crate::flops::Work;
use crate::instantiate::RELEASE;
use crate::query::DEFAULT_FRAME_SECS;
use crate::schedule;
use crate::typing::Value;
use node::Streamed;

pub const STREAMED: &str = "streamed";

#[derive(Clone, Debug, PartialEq)]
pub struct StreamConfig {
    pub block: usize,
    pub render: RenderConfig,
}

/// A target rendered from its range's start on, one block at a time, each node over the same
/// extent a whole render gives it: every block is the samples such a render writes there.
/// Each cut is decided as the stream reaches it, by the search a render runs before any sample.
pub struct Stream {
    graph: Graph,
    target: Expr,
    bindings: Vec<(String, f64)>,
    config: StreamConfig,
    shell: Render,
    audio: Vec<NodeId>,
    search: Search<'static>,
    nodes: Vec<Streamed>,
    root: usize,
    start: i64,
    at: i64,
    /// The range's end, or where the root, cut so far, ends where the range states none;
    /// `i64::MAX` while it is not cut, pulled on until its consumer stops.
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
        Stream::bound_to(graph, &bare, &bindings, config, None)
    }

    fn bound_to(
        graph: &Graph,
        target: &Expr,
        bindings: &[(String, f64)],
        config: StreamConfig,
        from: Option<&Checkpoint>,
    ) -> Result<Stream, EngineError> {
        if config.block == 0 {
            return Err(refusal("a block of no samples".to_string()));
        }
        let wrapped = bound(graph, target, bindings)?;
        let held = prepared(&wrapped, STREAMED)?;
        let rate = config.render.rate;
        sampled::on_the_grid(&held.tys, rate)?;
        let schedule = schedule::plan(&held.tys, &held.order, held.root, &[]);
        let audio = schedule.materialize.clone();
        let mut shell = Render::shell(held.tys, held.root, config.render.clone(), schedule);
        let support = Supports::new(&shell.tys, rate).of(shell.root);
        let range = shell.config.range;
        let start = range
            .start
            .unwrap_or_else(|| extent::default_start(support));
        let span = Span::of(&shell, &audio, start)?;
        let last = range.end.unwrap_or(span.root_end);
        reach::extend(&mut shell, &audio, Extent::new(start, last.max(start)))?;
        let frame = ((DEFAULT_FRAME_SECS * f64::from(rate)).round() as usize).max(1);
        let keep = match config.render.until {
            Some(_) => frame,
            None => 0,
        };
        let mut nodes = node::built(&shell, config.block, keep)?;
        let root = nodes
            .iter()
            .position(|n| n.id == shell.root)
            .ok_or_else(|| EngineError::UnknownNode(shell.tys.name(shell.root).to_string()))?;
        let at = match from {
            Some(checkpoint) => {
                if nodes.len() != checkpoint.nodes.len() {
                    let target = sva_ast::render_expr(target);
                    return Err(mismatch(&target, "a graph of another shape"));
                }
                for (node, held) in nodes.iter_mut().zip(&checkpoint.nodes) {
                    node.resume(&shell, held, checkpoint.at)?;
                }
                checkpoint.at
            }
            None => start,
        };
        let forms = Rc::new(Forms::new(
            Cow::Owned(shell.tys.clone()),
            Cow::Owned(shell.config.clone()),
        ));
        let search = Search::new(forms, &audio, shell.root, &span)?;
        let mut stream = Stream {
            graph: graph.clone(),
            target: target.clone(),
            bindings: bindings.to_vec(),
            config,
            shell,
            audio,
            search,
            nodes,
            root,
            start,
            at,
            last,
            frame,
            end: None,
            work: Work {
                waves: Some(0),
                ..Work::default()
            },
        };
        if let Some(checkpoint) = from {
            stream.walked(checkpoint)?;
        }
        stream.decide(at)?;
        Ok(stream)
    }

    /// Each solver's walk as the checkpoint left it, or where only its release moved, heard
    /// again from the chunks its site stepped, none of which its felt had landed in.
    fn walked(&mut self, checkpoint: &Checkpoint) -> Result<(), EngineError> {
        for (id, params, walk, ahead) in &checkpoint.walks {
            let Value::Solver(now) = self.shell.tys.value(*id) else {
                continue;
            };
            if self.search.bounds.of(*id)?.is_err() {
                continue;
            }
            let own = self.nodes.iter().find(|n| n.id == *id);
            let heard = own.and_then(|n| n.heard_to(checkpoint.at));
            let Some(tail) = self.search.bounds.solver_mut(*id) else {
                continue;
            };
            if **now == *params {
                tail.walk = walk.clone();
                tail.ahead = ahead.clone();
            } else if let (true, Some(heard)) = (now.differs_in_release_alone(params), heard) {
                tail.walk = Walk::before_landing(STEP, &heard);
            }
        }
        Ok(())
    }

    /// Every grid instant at or before `sample` decided, and the cuts found so far applied.
    fn decide(&mut self, sample: i64) -> Result<(), EngineError> {
        let through = (sample - self.search.bounds.grid.first).max(0) as usize / STEP;
        let before = self.search.cuts.len();
        self.search.decide(through, &Machines(&self.nodes))?;
        self.work.proofs = self.search.decided as u64;
        match self.search.cuts.len() == before {
            true => Ok(()),
            false => self.apply(),
        }
    }

    /// Each solver's walk hears its own machine; a copy played ahead is let go once caught up.
    fn feed(&mut self) {
        for solver in self.search.bounds.solvers() {
            let Some(node) = self.nodes.iter().find(|n| n.id == solver) else {
                continue;
            };
            let Some(tail) = self.search.bounds.solver_mut(solver) else {
                continue;
            };
            node.hear(&mut tail.walk);
            if tail
                .ahead
                .as_ref()
                .is_some_and(|ahead| ahead.tape.end() <= node.tape.end())
            {
                tail.ahead = None;
            }
        }
    }

    /// Every node's extent and support under the cuts decided so far, and where the root ends.
    fn apply(&mut self) -> Result<(), EngineError> {
        let rate = self.config.render.rate;
        let cuts = self.search.cuts.clone();
        let support = Supports::cut(&self.shell.tys, rate, cuts.clone()).of(self.shell.root);
        let range = self.config.render.range;
        self.last = range
            .end
            .or(extent::default_end(support))
            .unwrap_or(i64::MAX);
        self.shell.extents.cuts = cuts;
        let demand = Extent::new(self.start, self.last.max(self.start));
        reach::extend(&mut self.shell, &self.audio, demand)?;
        for node in &mut self.nodes {
            node.extent = self.shell.extents.of(node.id);
            node.support = self.shell.extents.support(node.id);
        }
        Ok(())
    }

    /// The next sample a cut is decided at or a site's state is read at.
    fn event(&self, at: i64) -> i64 {
        let first = self.search.bounds.grid.first;
        let step = STEP as i64;
        if self.search.done() {
            return i64::MAX;
        }
        let instant = first + ((at - first).div_euclid(step) + 1) * step;
        let walking = self
            .search
            .bounds
            .solvers()
            .into_iter()
            .any(|id| self.search.bounds.walking(id));
        let chunk = match walking {
            true => (at.div_euclid(step) + 1) * step,
            false => i64::MAX,
        };
        instant.min(chunk)
    }

    /// A level is known once its frame is whole, so it stops the stream no earlier than the
    /// block it becomes known in.
    fn settle(&mut self, from: i64, to: i64) {
        if self.end.is_some() {
            return;
        }
        if self.last <= to {
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
        let mut to = self.last.min(from + self.config.block as i64).max(from);
        let mut at = from;
        while at < to {
            let next = self.event(at).min(to);
            for n in 0..self.nodes.len() {
                let (done, rest) = self.nodes.split_at_mut(n);
                rest[0].run(&self.shell, done, from, next)?;
                let (priced, waves) = rest[0].work(at, next);
                self.work.priced_flops += priced;
                self.work.waves = self.work.waves.zip(waves).map(|(held, more)| held + more);
            }
            at = next;
            if at.rem_euclid(STEP as i64) == 0 {
                self.feed();
            }
            if (at - self.search.bounds.grid.first).rem_euclid(STEP as i64) == 0 {
                self.decide(at)?;
                to = to.min(self.last).max(at);
            }
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
        let bounds = &self.search.bounds;
        let walks = bounds
            .solvers()
            .into_iter()
            .filter_map(|id| {
                let tail = bounds.solver(id)?;
                Some((
                    id,
                    tail.params.clone(),
                    tail.walk.clone(),
                    tail.ahead.clone(),
                ))
            })
            .collect();
        Checkpoint {
            at: self.at,
            target: sva_ast::render_expr(&self.target),
            bindings: self.bindings.clone(),
            rate: self.config.render.rate,
            block: self.config.block,
            nodes: self.nodes.iter().map(Streamed::held).collect(),
            walks,
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
        let config = self.config.clone();
        Stream::bound_to(&self.graph, &self.target, &merged, config, Some(checkpoint))
    }
}

/// The stream's machines: a solver's walk waits on a copy of its own, played ahead.
struct Machines<'n>(&'n [Streamed]);

impl Sites for Machines<'_> {
    fn player(&self, solver: NodeId) -> Option<Played> {
        self.0.iter().find(|n| n.id == solver)?.played()
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

/// Every node's state at a block's end, each solver's walk, and what its stream was opened
/// with.
#[derive(Clone)]
pub struct Checkpoint {
    at: i64,
    target: String,
    bindings: Vec<(String, f64)>,
    rate: u32,
    block: usize,
    nodes: Vec<Option<node::NodeState>>,
    walks: Vec<(NodeId, Params, Walk, Option<Played>)>,
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
