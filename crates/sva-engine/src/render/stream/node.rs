// Concern: builds one streamed node, how far back its readers reach, and runs it over a block | Non-concern: the order nodes run in, bindings | IO: (NodeId) -> Streamed; (to) -> its tape

use std::collections::BTreeMap;

use sva_formula::{Held, NodeId};
use sva_samples::{Machine, NodeRenderer, Rows, Tape, Window};

use super::super::pointwise::{self, Point};
use super::super::{Render, collapse_refused, sampled};
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::refs;
use crate::typing::Value;

pub(super) enum Kind {
    Rows(Rows),
    Point(Box<Point>),
    Machine {
        machine: Machine,
        /// Each slot's node, as an index into the stream's nodes.
        reads: Vec<usize>,
    },
}

pub(super) struct Streamed {
    pub(super) id: NodeId,
    pub(super) kind: Kind,
    pub(super) width: usize,
    /// How many samples before a block's start any reader, itself included, reaches.
    pub(super) keep: usize,
    pub(super) tape: Tape,
}

/// Every materialized node in order, each with the reach its readers need.
pub(super) fn built(shell: &Render, block: usize) -> Result<Vec<Streamed>, EngineError> {
    let mut nodes: Vec<Streamed> = Vec::new();
    let mut index: BTreeMap<NodeId, usize> = BTreeMap::new();
    for &id in &shell.schedule.materialize {
        let Built {
            kind,
            width,
            reach,
            own,
        } = kind(shell, id, &nodes, &index)?;
        for (at, back) in reach {
            let read = &mut nodes[at];
            read.keep = read.keep.max(back);
        }
        index.insert(id, nodes.len());
        nodes.push(Streamed {
            id,
            kind,
            width,
            keep: own,
            tape: Tape::new(width, 0),
        });
    }
    for node in &mut nodes {
        node.tape = Tape::new(node.width, node.keep + block);
    }
    Ok(nodes)
}

/// A node, how far back it reads each earlier node, and how far back it reads itself.
struct Built {
    kind: Kind,
    width: usize,
    reach: Vec<(usize, usize)>,
    own: usize,
}

fn kind(
    shell: &Render,
    id: NodeId,
    nodes: &[Streamed],
    index: &BTreeMap<NodeId, usize>,
) -> Result<Built, EngineError> {
    match shell.tys.ty(id).held {
        Held::Frames => Err(no_stream(shell, id, "a short-time spectrum")),
        Held::Sampled => machine(shell, id, nodes, index),
        _ => closed_form(shell, id).map(|(kind, width)| Built {
            kind,
            width,
            reach: Vec::new(),
            own: 0,
        }),
    }
}

fn machine(
    shell: &Render,
    id: NodeId,
    nodes: &[Streamed],
    index: &BTreeMap<NodeId, usize>,
) -> Result<Built, EngineError> {
    if let Value::Cast(Cast::Istft, _) = shell.tys.value(id) {
        return Err(no_stream(shell, id, "an inverse short-time transform"));
    }
    let width_of = |r: NodeId| index.get(&r).map_or(1, |at| nodes[*at].width);
    let program = sampled::program_reading(shell, id, &width_of)?;
    let reads: Vec<usize> = program
        .reads
        .iter()
        .map(|r| index.get(r).copied().ok_or_else(|| unheld(shell, *r)))
        .collect::<Result<_, _>>()?;
    let (mut reach, mut own, mut ahead) = (Vec::new(), 0, false);
    leaves(&program.renderer, &mut |leaf| match leaf {
        NodeRenderer::Buffer { shift, .. } if *shift > 0 => ahead = true,
        NodeRenderer::Buffer { id, shift } => {
            reach.push((reads[id.0 as usize], shift.unsigned_abs() as usize));
        }
        NodeRenderer::SelfAt { steps } => own = own.max(*steps as usize),
        _ => {}
    });
    if ahead {
        return Err(no_stream(
            shell,
            id,
            "a read ahead of the sample it is taken at",
        ));
    }
    let machine = Machine::open(&program.renderer, &program.layout, shell.config.rate, 0.0)
        .map_err(|e| sampled::refused(shell, id, &e))?;
    Ok(Built {
        width: machine.width(),
        kind: Kind::Machine { machine, reads },
        reach,
        own,
    })
}

/// Every leaf under a renderer's operators: slots, its own past, constants, time, solvers.
fn leaves(renderer: &NodeRenderer, found: &mut dyn FnMut(&NodeRenderer)) {
    match renderer {
        NodeRenderer::Add(parts) | NodeRenderer::Mul(parts) | NodeRenderer::Join(parts) => {
            parts.iter().for_each(|p| leaves(p, found));
        }
        NodeRenderer::Sub(a, b)
        | NodeRenderer::Div(a, b)
        | NodeRenderer::Pow(a, b)
        | NodeRenderer::Zip(_, a, b) => {
            leaves(a, found);
            leaves(b, found);
        }
        NodeRenderer::Map(_, x)
        | NodeRenderer::Crop { x, .. }
        | NodeRenderer::Channel { x, .. } => leaves(x, found),
        NodeRenderer::Filter {
            x, cutoff, q, gain, ..
        } => [x, cutoff, q, gain]
            .into_iter()
            .for_each(|p| leaves(p, found)),
        leaf => found(leaf),
    }
}

/// `collapse_closed_form`'s choice between the rows and the point sampler.
fn closed_form(shell: &Render, id: NodeId) -> Result<(Kind, usize), EngineError> {
    let written = match refs::resolve(&shell.tys, id, 0, shell.tys.ty(id).held) {
        Ok(refs::Read::Substitute(form)) => Some(*form),
        _ => None,
    };
    let sum = refs::spectral_sum_of(&shell.tys, id, shell.tys.var(id));
    let (rate, profile) = (shell.config.rate, &shell.config.profile);
    let rows = match (&sum, &written) {
        (Err(_), None) => return point(shell, id),
        (Ok(sum), written) => Rows::of_spectral_sum_or_point(sum, written.as_ref(), rate, profile),
        (Err(_), Some(form)) => Rows::of(form, rate, profile),
    }
    .map_err(|e| match e {
        sva_samples::CollapseError::NoBlockRow => no_stream(shell, id, "a closed form in f"),
        e => collapse_refused(shell, id, &e),
    })?;
    let width = rows.width();
    Ok((Kind::Rows(rows), width))
}

fn point(shell: &Render, id: NodeId) -> Result<(Kind, usize), EngineError> {
    let tree = pointwise::plan(shell, id)?;
    if pointwise::reads_samples(&tree) {
        return Err(no_stream(
            shell,
            id,
            "a point-sampled form reading another node's samples",
        ));
    }
    let width = usize::from(shell.tys.ty(id).width).max(1);
    Ok((Kind::Point(Box::new(tree)), width))
}

impl Streamed {
    /// Samples up to `to`, every node it reads already there; `from` is the block's start.
    pub(super) fn run(
        &mut self,
        shell: &Render,
        done: &[Streamed],
        from: usize,
        to: usize,
    ) -> Result<(), EngineError> {
        self.tape.forget_before(from.saturating_sub(self.keep));
        let id = self.id;
        match &mut self.kind {
            Kind::Rows(rows) => rows
                .extend(to, &mut self.tape)
                .map_err(|e| collapse_refused(shell, id, &e)),
            Kind::Point(tree) => {
                let step = 1.0 / f64::from(shell.config.rate);
                for i in self.tape.end()..to {
                    for c in 0..self.width {
                        let v = pointwise::value(shell, tree, c, 0.0 + i as f64 * step)
                            .map_err(|e| pointwise::refused(shell, id, &e))?;
                        self.tape.push(c, v.re);
                    }
                }
                Ok(())
            }
            Kind::Machine { machine, reads } => {
                let windows: Vec<Window> = reads.iter().map(|&r| done[r].tape.window()).collect();
                machine
                    .run_to(to, &windows, &mut self.tape)
                    .map_err(|e| sampled::refused(shell, id, &e))
            }
        }
    }
}

pub(super) fn no_stream(shell: &Render, id: NodeId, class: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.no_stream".to_string(),
        message: format!(
            "`{}` is {class}, which no block of a stream reads on its own",
            shell.tys.name(id)
        ),
        location: Located::at(shell.tys.name(id), None),
        help: "render it to a stated end instead of streaming it".to_string(),
    })
}

fn unheld(shell: &Render, id: NodeId) -> EngineError {
    EngineError::UnknownNode(shell.tys.name(id).to_string())
}
