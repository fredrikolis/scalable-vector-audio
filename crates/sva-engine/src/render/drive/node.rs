// Concern: builds a driven node, its clock and its readers' reach, and runs it over a block | Non-concern: the order nodes run in, what an edit keeps | IO: (NodeId) -> Driven; (from, to) -> its tape

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
    Ended,
}

pub(in crate::render) struct Driven {
    pub(in crate::render) id: NodeId,
    pub(in crate::render) kind: Kind,
    /// A sample's price where no row prices it.
    per_sample: u128,
    pub(in crate::render) width: usize,
    /// How many samples before a block's start any reader, itself included, reaches.
    pub(in crate::render) keep: usize,
    /// How far behind the root's its own clock runs.
    pub(in crate::render) lag: i64,
    /// Each node it reads, as an index into the driver's nodes, and the shift.
    pub(in crate::render) reads: Vec<(usize, i64)>,
    pub(super) own: usize,
    trailing: bool,
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

/// Each of `order`, its clock and the reach its readers need.
pub(in crate::render) fn built(
    shell: &Render,
    order: &[NodeId],
    mut hold: Hold,
) -> Result<Vec<Driven>, EngineError> {
    let trailing = matches!(hold, Hold::Trailing { .. });
    let mut nodes: Vec<Driven> = Vec::new();
    let mut index: BTreeMap<NodeId, usize> = BTreeMap::new();
    for &id in order {
        let extent = shell.extents.of(id);
        let support = shell.extents.support(id);
        index.insert(id, nodes.len());
        if let Hold::Every(held) = &mut hold
            && let Some(buffer) = held.remove(&id)
        {
            nodes.push(whole(id, buffer, extent, support));
            continue;
        }
        let Built {
            kind,
            width,
            reads,
            own,
        } = kind(shell, id, &nodes, &index)?;
        let per_sample = match kind {
            Kind::Whole | Kind::Rows(_) | Kind::Ended => 0,
            Kind::Point(_) | Kind::Machine { .. } => crate::flops::per_sample(shell, id),
        };
        nodes.push(Driven {
            id,
            kind,
            per_sample,
            width,
            keep: 0,
            lag: 0,
            reads,
            own,
            trailing,
            extent,
            support,
            tape: Tape::new(width, 0, extent.start),
        });
    }
    let root = index.get(&shell.root).copied();
    clocked(&mut nodes, root, trailing);
    if let Hold::Trailing { block, root_keep } = hold {
        for (at, node) in nodes.iter_mut().enumerate() {
            if Some(at) == root {
                node.keep = node.keep.max(root_keep);
            }
            node.tape = Tape::new(node.width, node.keep + block, node.extent.start);
        }
    }
    Ok(nodes)
}

/// Each node's lag, the least its readers need, and its keep, the most they reach back.
fn clocked(nodes: &mut [Driven], root: Option<usize>, trailing: bool) {
    let mut lags: Vec<Option<i64>> = vec![None; nodes.len()];
    if let Some(root) = root {
        lags[root] = Some(0);
    }
    for at in (0..nodes.len()).rev() {
        if trailing && (lags[at].is_none() || nodes[at].extent.is_empty()) {
            nodes[at].kind = Kind::Ended;
            nodes[at].reads.clear();
        }
        let lag = lags[at].unwrap_or(0);
        nodes[at].lag = lag;
        nodes[at].keep = nodes[at].own;
        for &(read, shift) in &nodes[at].reads {
            let wants = lag - shift;
            lags[read] = Some(lags[read].map_or(wants, |held| held.min(wants)));
        }
    }
    for at in 0..nodes.len() {
        for (read, shift) in nodes[at].reads.clone() {
            let back = (nodes[at].lag - shift - nodes[read].lag).max(0) as usize;
            nodes[read].keep = nodes[read].keep.max(back);
        }
    }
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
        keep: 0,
        lag: 0,
        reads: Vec::new(),
        own: 0,
        trailing: false,
        extent,
        support,
        tape: Tape::from(buffer),
    }
}

struct Built {
    kind: Kind,
    width: usize,
    reads: Vec<(usize, i64)>,
    pub(super) own: usize,
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
            reads: Vec::new(),
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
    let slots: Vec<usize> = program
        .reads
        .iter()
        .map(|r| index.get(r).copied().ok_or_else(|| unheld(shell, *r)))
        .collect::<Result<_, _>>()?;
    let (mut reads, mut own, mut ahead) = (Vec::new(), 0, false);
    crate::render::extent::leaves(&program.renderer, &mut |leaf| match leaf {
        NodeRenderer::Buffer { shift, .. } if *shift > 0 => ahead = true,
        NodeRenderer::Buffer { id, shift } => reads.push((slots[id.0 as usize], *shift)),
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
    let live: Vec<Extent> = slots
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
        kind: Kind::Machine {
            machine,
            reads: slots,
        },
        reads,
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
    /// Samples up to `to` on its own clock, every node it reads already there; the span it
    /// computed.
    pub(in crate::render) fn run(
        &mut self,
        shell: &Render,
        done: &[Driven],
        from: i64,
        to: i64,
    ) -> Result<(i64, i64), EngineError> {
        if self.forgets() {
            self.tape.forget_before(from - self.keep as i64);
        }
        let to = to.min(self.extent.end);
        let start = self.tape.end();
        if self.extent.is_empty() || to <= start {
            return Ok((start, start));
        }
        let id = self.id;
        match &mut self.kind {
            Kind::Whole | Kind::Ended => {}
            Kind::Rows(rows) => rows
                .extend(to, &mut self.tape)
                .map_err(|e| collapse_refused(shell, id, &e))?,
            Kind::Point(tree) => {
                let step = 1.0 / f64::from(shell.config.rate);
                for n in self.tape.end()..to {
                    for c in 0..self.width {
                        let v = pointwise::value(shell, tree, c, n as f64 * step)
                            .map_err(|e| pointwise::refused(shell, id, &e))?;
                        self.tape.push(c, v.re);
                    }
                }
            }
            Kind::Machine { machine, reads } => {
                let windows: Vec<Window> = reads
                    .iter()
                    .map(|&r| done[r].tape.within(done[r].support))
                    .collect();
                machine
                    .run_to(to, &windows, &mut self.tape)
                    .map_err(|e| sampled::refused(shell, id, &e))?;
            }
        }
        Ok((start, self.tape.end()))
    }

    pub(super) fn forgets(&self) -> bool {
        self.trailing
    }

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
            Kind::Whole | Kind::Machine { .. } | Kind::Ended => (self.per_sample * n, Some(0)),
        }
    }

    pub(in crate::render) fn spent(&self, local: i64) -> bool {
        !matches!(self.kind, Kind::Ended)
            && !self.extent.is_empty()
            && local - self.keep as i64 >= self.extent.end
    }

    pub(in crate::render) fn end(&mut self) {
        self.kind = Kind::Ended;
        self.tape = Tape::new(self.width, 0, self.extent.end);
    }

    /// Held only where its tape ends at `at`; a machine with no call site holds none.
    pub(in crate::render) fn state_at(&self, at: i64) -> Option<MachineState> {
        let Kind::Machine { machine, .. } = &self.kind else {
            return None;
        };
        let end = self.tape.end();
        (end == at || (!machine.stateful() && end >= at)).then(|| machine.state())
    }

    pub(in crate::render) fn bytes(&self) -> usize {
        let tape = self.tape.capacity() * self.tape.width() * size_of::<f64>();
        let state = match &self.kind {
            Kind::Machine { machine, .. } => machine.bytes(),
            _ => 0,
        };
        tape + state
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
