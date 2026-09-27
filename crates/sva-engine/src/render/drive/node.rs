// Concern: builds one driven node, how far back its readers reach, and runs it over a block | Non-concern: the order nodes run in, bindings | IO: (NodeId) -> Driven; (to) -> its tape

use std::collections::BTreeMap;

use sva_formula::{Held, NodeId};
use sva_samples::{Buffer, Extent, Machine, MachineState, NodeRenderer, Rows, Tape, Window};

use super::super::pointwise::{self, Point};
use super::super::{Render, collapse_refused, sampled};
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::refs;
use crate::typing::Value;

pub(in crate::render) enum Kind {
    /// Held whole before any reader ran.
    Whole,
    Rows(Rows),
    Point(Box<Point>),
    Machine {
        machine: Machine,
        /// Each slot's node, as an index into the driver's nodes.
        reads: Vec<usize>,
    },
}

pub(in crate::render) struct Driven {
    pub(in crate::render) id: NodeId,
    pub(in crate::render) kind: Kind,
    /// A sample's price where no row prices it.
    per_sample: u128,
    pub(in crate::render) width: usize,
    /// How many samples before a block's start any reader, itself included, reaches; `None`
    /// keeps every one.
    pub(in crate::render) keep: Option<usize>,
    /// What its readers alone reach, which a checkpoint has to hold.
    read_back: usize,
    pub(in crate::render) extent: Extent,
    pub(in crate::render) support: Extent,
    pub(in crate::render) tape: Tape,
}

/// How a driver holds its nodes' samples.
pub(in crate::render) enum Hold {
    /// A block, and as far back as its readers reach; the root `root_keep` samples besides.
    Trailing { block: usize, root_keep: usize },
    /// Every sample, the nodes held here whole before any reader runs.
    Every(BTreeMap<NodeId, Buffer>),
}

/// Each of `order`, with the reach its readers need.
pub(in crate::render) fn built(
    shell: &Render,
    order: &[NodeId],
    mut hold: Hold,
) -> Result<Vec<Driven>, EngineError> {
    let mut nodes: Vec<Driven> = Vec::new();
    let mut index: BTreeMap<NodeId, usize> = BTreeMap::new();
    for &id in order {
        let extent = shell.extents.of(id);
        let support = shell.extents.support(id);
        if let Hold::Every(held) = &mut hold
            && let Some(buffer) = held.remove(&id)
        {
            index.insert(id, nodes.len());
            nodes.push(whole(id, buffer, extent, support));
            continue;
        }
        let Built {
            kind,
            width,
            reach,
            own,
        } = kind(shell, id, &nodes, &index)?;
        for (at, back) in reach {
            let read = &mut nodes[at];
            read.keep = read.keep.map(|keep| keep.max(back));
            read.read_back = read.read_back.max(back);
        }
        index.insert(id, nodes.len());
        let per_sample = match kind {
            Kind::Whole | Kind::Rows(_) => 0,
            Kind::Point(_) | Kind::Machine { .. } => crate::flops::per_sample(shell, id),
        };
        let (keep, capacity) = match &hold {
            Hold::Trailing { root_keep, .. } if id == shell.root => (Some(own.max(*root_keep)), 0),
            Hold::Trailing { .. } => (Some(own), 0),
            Hold::Every(_) => (None, extent.len()),
        };
        nodes.push(Driven {
            id,
            kind,
            per_sample,
            width,
            keep,
            read_back: own,
            extent,
            support,
            tape: Tape::new(width, capacity, extent.start),
        });
    }
    if let Hold::Trailing { block, .. } = hold {
        for node in &mut nodes {
            let keep = node.keep.unwrap_or(0);
            node.tape = Tape::new(node.width, keep + block, node.extent.start);
        }
    }
    Ok(nodes)
}

/// A node held whole before any reader runs.
pub(in crate::render) fn whole(
    id: NodeId,
    buffer: Buffer,
    extent: Extent,
    support: Extent,
) -> Driven {
    Driven {
        id,
        kind: Kind::Whole,
        per_sample: 0,
        width: buffer.planes.len().max(1),
        keep: None,
        read_back: 0,
        extent,
        support,
        tape: Tape::from(buffer),
    }
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
    nodes: &[Driven],
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
    nodes: &[Driven],
    index: &BTreeMap<NodeId, usize>,
) -> Result<Built, EngineError> {
    if let Value::Cast(Cast::Istft, _) = shell.tys.value(id) {
        return Err(no_stream(shell, id, "an inverse short-time transform"));
    }
    let width_of = |r: NodeId| index.get(&r).map_or(1, |at| nodes[*at].width);
    let extent = shell.extents.of(id);
    let program = sampled::program_reading(shell, id, &width_of)?;
    let reads: Vec<usize> = program
        .reads
        .iter()
        .map(|r| index.get(r).copied().ok_or_else(|| unheld(shell, *r)))
        .collect::<Result<_, _>>()?;
    let (mut reach, mut own, mut ahead) = (Vec::new(), 0, false);
    crate::render::extent::leaves(&program.renderer, &mut |leaf| match leaf {
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
    let live: Vec<Extent> = reads
        .iter()
        .map(|&r| match nodes[r].kind {
            Kind::Whole => {
                let tape = &nodes[r].tape;
                sampled::live(tape.planes(), tape.base(), nodes[r].support)
            }
            _ => nodes[r].support,
        })
        .collect();
    let rate = shell.config.rate;
    let span = (extent.start, extent.end);
    let machine = Machine::live(&program.renderer, &program.layout, rate, span, &live)
        .map_err(|e| sampled::refused(shell, id, &e))?;
    Ok(Built {
        width: machine.width(),
        kind: Kind::Machine { machine, reads },
        reach,
        own,
    })
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

impl Driven {
    /// Samples up to `to` within its extent, every node it reads already there; `from` is the
    /// block's start.
    pub(in crate::render) fn run(
        &mut self,
        shell: &Render,
        done: &[Driven],
        from: i64,
        to: i64,
    ) -> Result<(), EngineError> {
        if let Some(keep) = self.keep {
            self.tape.forget_before(from - keep as i64);
        }
        let to = to.min(self.extent.end);
        if self.extent.is_empty() || to <= self.tape.end() {
            return Ok(());
        }
        let id = self.id;
        match &mut self.kind {
            Kind::Whole => Ok(()),
            Kind::Rows(rows) => rows
                .extend(to, &mut self.tape)
                .map_err(|e| collapse_refused(shell, id, &e)),
            Kind::Point(tree) => {
                let step = 1.0 / f64::from(shell.config.rate);
                for n in self.tape.end()..to {
                    for c in 0..self.width {
                        let v = pointwise::value(shell, tree, c, n as f64 * step)
                            .map_err(|e| pointwise::refused(shell, id, &e))?;
                        self.tape.push(c, v.re);
                    }
                }
                Ok(())
            }
            Kind::Machine { machine, reads, .. } => {
                let windows: Vec<Window> = reads
                    .iter()
                    .map(|&r| done[r].tape.within(done[r].support))
                    .collect();
                machine
                    .run_to(to, &windows, &mut self.tape)
                    .map_err(|e| sampled::refused(shell, id, &e))
            }
        }
    }
}

impl Driven {
    /// `(priced flops, waves)` over `[from, to)` of its extent: a pointwise tree's waves go
    /// uncounted.
    pub(in crate::render) fn work(&self, from: i64, to: i64) -> (u128, Option<u128>) {
        let (from, to) = (from.max(self.extent.start), to.min(self.extent.end));
        let n = (to - from).max(0) as u128;
        match &self.kind {
            Kind::Rows(rows) => {
                let (priced, waves) = rows.work(from, to.max(from));
                (priced, Some(waves))
            }
            Kind::Point(_) => (self.per_sample * n, None),
            Kind::Whole | Kind::Machine { .. } => (self.per_sample * n, Some(0)),
        }
    }
}

/// A sampled node's state and the samples its readers reach back to; a closed form holds none.
#[derive(Clone)]
pub(in crate::render) struct NodeState {
    tape: Tape,
    state: MachineState,
}

impl Driven {
    pub(in crate::render) fn held(&self) -> Option<NodeState> {
        match &self.kind {
            Kind::Machine { machine, .. } => Some(NodeState {
                tape: self.tape.clone(),
                state: machine.state(),
            }),
            _ => None,
        }
    }

    /// Takes `held` at sample `at`; a closed form reads its own past again instead.
    pub(in crate::render) fn resume(
        &mut self,
        shell: &Render,
        held: &Option<NodeState>,
        at: i64,
    ) -> Result<(), EngineError> {
        let from = (at - self.keep.unwrap_or(0) as i64).max(self.extent.start);
        if self.extent.is_empty() {
            return Ok(());
        }
        match (&mut self.kind, held) {
            (Kind::Machine { machine, .. }, Some(held))
                if held.tape.width() == self.width
                    && held.tape.base() <= (at - self.read_back as i64).max(self.extent.start)
                    && held.tape.end() == at.min(self.extent.end) =>
            {
                machine
                    .carry(&held.state)
                    .map_err(|e| sampled::refused(shell, self.id, &e))?;
                self.tape = held.tape.clone();
                Ok(())
            }
            (Kind::Rows(_) | Kind::Point(_), None) => {
                self.tape = Tape::new(self.width, self.tape.capacity(), from);
                self.run(shell, &[], at, at)
            }
            _ => Err(EngineError::refused(Diagnostic {
                code: "engine.checkpoint_mismatch".to_string(),
                message: format!(
                    "`{}` holds another kind of state than this checkpoint",
                    shell.tys.name(self.id)
                ),
                location: Located::at(shell.tys.name(self.id), None),
                help: "resume a checkpoint on the stream it was taken of".to_string(),
            })),
        }
    }
}

pub(in crate::render) fn no_stream(shell: &Render, id: NodeId, class: &str) -> EngineError {
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
