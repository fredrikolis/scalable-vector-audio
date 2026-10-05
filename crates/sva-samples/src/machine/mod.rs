// Concern: runs one node renderer a block at a time, span after span, onto its own samples | Non-concern: the op array's shape (ops.rs), cutting spans (live.rs) | IO: (Spanned, reads, own) -> samples

mod block;
mod kernels;
mod live;
pub mod ops;
mod read;
pub mod renderer;
mod tape;

use std::sync::Arc;

use crate::buffer::{Buffer, SampleView};
use crate::collapse::Program;
use crate::error::SampleError;
use crate::filters::FilterSite;
use crate::grid::Extent;
use crate::grid::Grid;
use crate::physics::{Solver, site};
use block::{BLOCK, BlockScratch};
use ops::{Layout, Op, lowered};
use read::Here;
use renderer::{Formula, Index, NodeRenderer, Site, Slot};

pub use live::{Span, Spanned};

pub use ops::Layout as MachineLayout;

/// The op array, each op's width and operand slots, the formulas its ops name and the call
/// sites the run opens state for.
#[derive(Clone)]
pub(super) struct CompiledOps {
    ops: Vec<Op>,
    widths: Vec<usize>,
    args: Vec<Vec<usize>>,
    /// Each formula, a written one with its program per component.
    formulas: Vec<(Formula, Arc<Vec<Program>>)>,
    indices: Vec<Index<usize>>,
    sites: Vec<Site>,
    pub width: usize,
}

impl NodeRenderer {
    pub(super) fn compile(&self, layout: &Layout) -> Result<CompiledOps, SampleError> {
        let (lowered, width) = lowered(self, layout)?;
        let mut formulas: Vec<(Formula, Arc<Vec<Program>>)> = lowered
            .formulas
            .into_iter()
            .map(|f| (f, Arc::new(Vec::new())))
            .collect();
        for (slot, op) in lowered.ops.iter().enumerate() {
            if let Op::Formula { at } = op
                && let (Formula::Written(w), programs) = &mut formulas[*at]
            {
                let each = (0..lowered.widths[slot]).map(|c| Program::of(&[&w.body], &w.refs, c));
                *programs = Arc::new(each.collect());
            }
        }
        Ok(CompiledOps {
            ops: lowered.ops,
            widths: lowered.widths,
            args: lowered.args,
            formulas,
            indices: lowered.indices,
            sites: layout.sites.clone(),
            width,
        })
    }
}

#[derive(Clone)]
enum State {
    Filter(FilterSite),
    Physics(Box<dyn Solver>),
}

impl Clone for Box<dyn Solver> {
    fn clone(&self) -> Box<dyn Solver> {
        self.boxed()
    }
}

/// A filter site carries one lane per component of its widest argument, which the compiled
/// slot width already states.
fn open(p: &CompiledOps, grid: Grid) -> Result<Vec<State>, SampleError> {
    let mut lanes = vec![1usize; p.sites.len()];
    for (slot, op) in p.ops.iter().enumerate() {
        if let Op::Filter { site, .. } = op {
            lanes[site.0 as usize] = p.widths[slot];
        }
    }
    p.sites
        .iter()
        .zip(lanes)
        .map(|(s, width)| {
            Ok(match s {
                Site::Filter(shape) => State::Filter(FilterSite::new(
                    *shape,
                    width,
                    &[0.0],
                    &[0.0],
                    &[0.0],
                    grid.sr(),
                )),
                Site::Physics(params) => State::Physics(site(params, grid.sr())?),
            })
        })
        .collect()
}

/// Component `c` of an operand that may be mono where its neighbour is wide.
fn part(v: &[f64], c: usize) -> f64 {
    v[c.min(v.len() - 1)]
}

/// A node's own samples, and the sample before which its past is silent.
pub type Own<'a> = (&'a mut Buffer, i64);

/// One compiled node and every call site's state, run over any span of the grid in order.
/// A span continues exactly where the last ended, so blocks write the samples one run would.
pub struct Machine {
    compiled_ops: CompiledOps,
    states: Vec<State>,
    block: BlockScratch,
    grid: Grid,
    /// Each later span's first sample and the program it runs, the next one last.
    ahead: Vec<(i64, CompiledOps)>,
}

#[derive(Clone)]
pub struct MachineState {
    sites: Vec<Site>,
    states: Vec<State>,
}

impl State {
    fn bytes(&self) -> usize {
        match self {
            State::Filter(filter) => filter.bytes(),
            State::Physics(solver) => solver.bytes(),
        }
    }
}

impl MachineState {
    pub fn bytes(&self) -> usize {
        let states: usize = self.states.iter().map(State::bytes).sum();
        size_of::<Self>() + std::mem::size_of_val(self.sites.as_slice()) + states
    }
}

impl Machine {
    /// Stepping from `at`, each span's own program from where it starts.
    pub fn over(spanned: &Spanned, at: i64) -> Result<Machine, SampleError> {
        let grid = spanned.grid();
        let mut ahead: Vec<(i64, CompiledOps)> = spanned
            .compiled()
            .iter()
            .filter(|(span, _)| span.to > at)
            .map(|(span, compiled_ops)| (span.from, compiled_ops.clone()))
            .collect();
        ahead.reverse();
        let first = match ahead.pop() {
            Some((_, compiled_ops)) => compiled_ops,
            None => spanned.silent()?,
        };
        let states = open(&first, grid)?;
        let block = BlockScratch::of(&first.widths);
        Ok(Machine {
            compiled_ops: first,
            states,
            block,
            grid,
            ahead,
        })
    }

    pub fn width(&self) -> usize {
        self.compiled_ops.width
    }

    pub fn stateful(&self) -> bool {
        !self.compiled_ops.sites.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.states.iter().map(State::bytes).sum()
    }

    pub fn run_to(&mut self, to: i64, reads: &[SampleView], own: Own) -> Result<(), SampleError> {
        for state in &mut self.states {
            if let State::Filter(filter) = state {
                filter.forget_frames();
            }
        }
        self.steps(to, reads, own)
    }

    /// `run_to`, the filters' frames kept.
    pub fn run_on(&mut self, to: i64, reads: &[SampleView], own: Own) -> Result<(), SampleError> {
        self.steps(to, reads, own)
    }

    fn steps(
        &mut self,
        to: i64,
        reads: &[SampleView],
        (own, origin): Own,
    ) -> Result<(), SampleError> {
        loop {
            while let Some((from, _)) = self.ahead.last()
                && *from <= own.end()
            {
                let (_, compiled_ops) = self.ahead.pop().expect("a span ahead");
                self.block = BlockScratch::of(&compiled_ops.widths);
                self.compiled_ops = compiled_ops;
            }
            let until = self.ahead.last().map_or(to, |(from, _)| (*from).min(to));
            self.stepped(until, reads, (own, origin))?;
            if own.end() >= to {
                return Ok(());
            }
        }
    }

    fn stepped(
        &mut self,
        to: i64,
        reads: &[SampleView],
        (own, origin): Own,
    ) -> Result<(), SampleError> {
        let p = &self.compiled_ops;
        while own.end() < to {
            let from = own.end();
            let most = (to - from).min(BLOCK as i64 - from.rem_euclid(BLOCK as i64)) as usize;
            let len = p.block(from, most, &mut self.block.recurrent);
            let here = Here {
                reads,
                own: own.within(Extent::from(origin)),
                grid: self.grid,
            };
            let (held, refused) =
                block::run(p, &mut self.block, &here, &mut self.states, (from, len));
            for i in 0..held {
                let top = self.block.top(p, i);
                for c in 0..p.width {
                    own.push(c, part(top, c));
                }
            }
            if let Some(e) = refused {
                return Err(e);
            }
        }
        Ok(())
    }

    /// Call site `site`'s solver as it stands now, where that site is one.
    pub fn solver(&self, site: usize) -> Option<&dyn Solver> {
        match self.states.get(site)? {
            State::Physics(solver) => Some(solver.as_ref()),
            State::Filter(_) => None,
        }
    }

    pub fn filter(&self, site: usize) -> Option<&FilterSite> {
        match self.states.get(site)? {
            State::Filter(filter) => Some(filter),
            State::Physics(_) => None,
        }
    }

    pub fn state(&self) -> MachineState {
        MachineState {
            sites: self.compiled_ops.sites.clone(),
            states: self.states.clone(),
        }
    }

    pub fn restart(&mut self) {
        self.states = open(&self.compiled_ops, self.grid).expect("the sites opened once already");
    }

    /// Whether `carry` takes `held`: the same sites, a varying parameter's values aside.
    pub fn accepts(&self, held: &MachineState) -> bool {
        held.sites == self.compiled_ops.sites
    }

    /// Takes `held`'s state whole where its sites are these; `false`, and nothing taken, else.
    pub fn carry(&mut self, held: &MachineState) -> bool {
        let taken = self.accepts(held);
        if taken {
            self.states.clone_from(&held.states);
        }
        taken
    }
}

impl CompiledOps {
    /// Up to `most` samples from `from`, and the slots run sample by sample: each reading its
    /// own past inside the block, and what reads one.
    fn block(&self, from: i64, most: usize, recurrent: &mut Vec<bool>) -> usize {
        let reach: Vec<Option<usize>> = self
            .ops
            .iter()
            .map(|op| own_reach(op, from, most))
            .collect();
        let len = reach
            .iter()
            .flatten()
            .filter(|n| **n >= RUN)
            .fold(most, |len, n| len.min(*n))
            .max(1);
        recurrent.clear();
        for (slot, reach) in reach.iter().enumerate() {
            let reads = self.args[slot].iter().any(|a| recurrent[*a]);
            recurrent.push(reads || reach.is_some_and(|n| n < len));
        }
        len
    }
}

/// How long a block from `from` may run with an own-past read reading only before it.
fn own_reach(op: &Op, from: i64, most: usize) -> Option<usize> {
    match op {
        Op::Read {
            slot: Slot::Own,
            at,
        }
        | Op::ReadScaled {
            slot: Slot::Own,
            at,
            ..
        } => {
            let last = |len: usize| from + len as i64 - 1;
            let mut len = most;
            while len > 1 && at.at(from).max(at.at(last(len))) >= from {
                len /= 2;
            }
            Some(match at.at(from).max(at.at(last(len))) < from {
                true => len,
                false => 0,
            })
        }
        Op::Indexed {
            slot: Slot::Own,
            reach,
            ..
        } => Some(match reach {
            Some((_, most)) if *most < 0 => {
                usize::try_from(most.unsigned_abs()).unwrap_or(usize::MAX)
            }
            _ => 0,
        }),
        _ => None,
    }
}

/// The fewest samples a block an own-past read shortens runs; a nearer read runs per sample.
const RUN: usize = 16;

#[cfg(test)]
mod tests {
    use super::renderer::{BufId, Index, Map, NodeRenderer, Slot};
    use super::*;
    use crate::grid::{Extent, Round, WIDE};

    const LONG: [i64; 2] = [1489, 1721];

    /// A reverb's loop: two lines fed back through a mix, each damped by its last sample.
    fn feedback() -> NodeRenderer {
        let own = |lag: i64, k: usize| NodeRenderer::Channel {
            x: Box::new(NodeRenderer::Read {
                slot: Slot::Own,
                map: Map::shift(-lag),
            }),
            k,
        };
        let input = NodeRenderer::Read {
            slot: Slot::Read(BufId(0)),
            map: Map::shift(0),
        };
        let line = |k: usize| {
            let mix = LONG
                .iter()
                .enumerate()
                .map(|(j, lag)| NodeRenderer::Mul(vec![NodeRenderer::Const(0.6), own(*lag, j)]));
            let fed = NodeRenderer::Add(std::iter::once(input.clone()).chain(mix).collect());
            NodeRenderer::Add(vec![
                NodeRenderer::Mul(vec![NodeRenderer::Const(0.7), fed]),
                NodeRenderer::Mul(vec![NodeRenderer::Const(0.2), own(1, k)]),
            ])
        };
        NodeRenderer::Join(vec![line(0), line(1)])
    }

    fn ran(to: i64) -> Vec<Vec<f64>> {
        let layout = Layout {
            grid: Grid::of(48_000),
            width: 2,
            read_widths: vec![1],
            sites: Vec::new(),
        };
        let spanned =
            Spanned::new(&feedback(), &layout, (0, to), &[Extent::EVERYWHERE]).expect("a program");
        let mut input = Buffer::empty(48_000, 1, to as usize, 0);
        (0..to).for_each(|n| input.push(0, f64::from(u8::from(n % 4800 == 0))));
        let mut out = Buffer::empty(48_000, 2, to as usize, 0);
        let mut machine = Machine::over(&spanned, 0).expect("a machine");
        machine
            .run_to(to, &[input.within(Extent::from(0))], (&mut out, 0))
            .expect("samples");
        out.planes
    }

    /// Each line reads both long lines and its own last sample, and both mix the same input:
    /// every identical read and mix is one op, writing the bits the loop's own arithmetic does.
    #[test]
    fn identical_reads_of_a_loops_own_past_run_once() {
        let to = 24_000;
        let layout = Layout {
            grid: Grid::of(48_000),
            width: 2,
            read_widths: vec![1],
            sites: Vec::new(),
        };
        assert_eq!(lowered(&feedback(), &layout).expect("ops").0.ops.len(), 20);
        let planes = ran(to);
        let mut y = vec![vec![0.0f64; to as usize]; 2];
        let past = |y: &[Vec<f64>], k: usize, n: i64| match n {
            n if n < 0 => 0.0,
            n => y[k][n as usize],
        };
        for n in 0..to {
            let input = f64::from(u8::from(n % 4800 == 0));
            let mut fed = 0.0 + input;
            for (j, lag) in LONG.iter().enumerate() {
                fed += 1.0 * 0.6 * past(&y, j, n - lag);
            }
            for k in 0..2 {
                y[k][n as usize] = 0.0 + 1.0 * 0.7 * fed + 1.0 * 0.2 * past(&y, k, n - 1);
            }
        }
        let bits = |p: &[Vec<f64>]| -> Vec<Vec<u64>> {
            p.iter()
                .map(|c| c.iter().map(|v| v.to_bits()).collect())
                .collect()
        };
        assert_eq!(bits(&planes), bits(&y));
    }

    /// A two-line loop whose last samples move a filter, a map, a pair, a crop, a draw, a
    /// delay read at a moving length and that length's instant.
    fn through_every_op() -> NodeRenderer {
        use super::renderer::{Binary, Formula, Unary};
        use NodeRenderer::{Const, Map as Of, Mul};
        let own = |lag: i64, k: usize| NodeRenderer::Channel {
            x: Box::new(NodeRenderer::Read {
                slot: Slot::Own,
                map: Map::shift(-lag),
            }),
            k,
        };
        let input = NodeRenderer::Read {
            slot: Slot::Read(BufId(0)),
            map: Map::shift(0),
        };
        let b = Box::new;
        let filtered = NodeRenderer::Filter {
            site: super::renderer::SiteId(0),
            from: 0,
            x: b(NodeRenderer::Add(vec![
                input.clone(),
                Mul(vec![Const(0.5), own(1, 0)]),
            ])),
            cutoff: b(NodeRenderer::Add(vec![
                Const(800.0),
                Mul(vec![Const(400.0), Of(Unary::Sin, b(own(1, 1)))]),
            ])),
            q: b(Const(0.7)),
            gain: b(Const(0.0)),
        };
        let paired = NodeRenderer::Add(vec![
            NodeRenderer::Zip(
                Binary::Max,
                b(NodeRenderer::Div(b(own(2, 0)), b(Const(3.0)))),
                b(own(1, 1)),
            ),
            NodeRenderer::Pow(b(Const(0.9)), b(own(1, 0))),
        ]);
        let cropped = NodeRenderer::Crop {
            x: b(NodeRenderer::Sub(b(paired), b(Const(0.5)))),
            window: (100, 20_000),
            a: 100.0 / 48_000.0,
            b: 20_000.0 / 48_000.0,
            rise: 0.01,
            fall: 0.01,
        };
        let drawn = NodeRenderer::Formula {
            formula: Formula::Drawn {
                seed: 7,
                rate: 1_000,
            },
            width: 1,
            time: b(NodeRenderer::Add(vec![
                NodeRenderer::Time,
                Mul(vec![Const(0.000_1), own(1, 0)]),
            ])),
        };
        let back = || {
            let length = NodeRenderer::Add(vec![
                Const(0.000_2),
                Mul(vec![Const(0.000_1), Of(Unary::Sin, b(own(1, 1)))]),
            ]);
            Index::Add(vec![
                Index::At(Map::whole(1, 0)),
                Index::Neg(Box::new(Index::Step(b(length), Round::Floor))),
            ])
        };
        let delayed = NodeRenderer::Indexed {
            slot: Slot::Own,
            index: back(),
            reach: Some((-14, -4)),
        };
        let lag = NodeRenderer::Sub(b(NodeRenderer::Time), b(NodeRenderer::Instant(back())));
        NodeRenderer::Join(vec![
            NodeRenderer::Add(vec![
                Mul(vec![Const(0.3), filtered]),
                Mul(vec![Const(0.2), cropped]),
                Mul(vec![Const(0.1), drawn]),
                Mul(vec![
                    Const(0.3),
                    NodeRenderer::Channel {
                        x: b(delayed),
                        k: 1,
                    },
                ]),
            ]),
            NodeRenderer::Add(vec![
                Mul(vec![Const(0.5), input]),
                Mul(vec![Const(0.4), own(1, 1)]),
                Mul(vec![Const(10.0), lag]),
            ]),
        ])
    }

    /// A machine's spans continue exactly where the last ended, whatever they cut a loop
    /// through every op into: the same bits run whole, a sample at a time, or 37 at a time.
    #[test]
    fn a_loop_through_every_op_writes_what_a_sample_at_a_time_run_does() {
        let to = 12_000;
        let layout = Layout {
            grid: Grid::of(48_000),
            width: 2,
            read_widths: vec![1],
            sites: vec![Site::Filter(sva_formula::Shape::Lowpass)],
        };
        let spanned = Spanned::new(&through_every_op(), &layout, (0, to), &[Extent::EVERYWHERE])
            .expect("a program");
        let mut input = Buffer::empty(48_000, 1, to as usize, 0);
        (0..to).for_each(|n| input.push(0, f64::from(u8::from(n % 2400 == 0))));
        let run = |step: i64| {
            let mut out = Buffer::empty(48_000, 2, to as usize, 0);
            let mut machine = Machine::over(&spanned, 0).expect("a machine");
            let mut at = 0;
            while at < to {
                at = (at + step).min(to);
                machine
                    .run_to(at, &[input.within(Extent::from(0))], (&mut out, 0))
                    .expect("samples");
            }
            let bits = out.planes.iter().flatten().map(|v| v.to_bits());
            bits.collect::<Vec<u64>>()
        };
        let whole = run(to);
        assert!(whole.iter().all(|v| f64::from_bits(*v).is_finite()));
        assert_eq!(whole, run(1));
        assert_eq!(whole, run(37));
    }

    /// A delay read at a moving length and a loop reading itself far back, by index.
    fn delayed() -> NodeRenderer {
        let now = Index::At(Map::whole(1, 0));
        let length = NodeRenderer::Add(vec![
            NodeRenderer::Const(0.002),
            NodeRenderer::Mul(vec![
                NodeRenderer::Const(0.000_2),
                NodeRenderer::Map(
                    super::renderer::Unary::Sin,
                    Box::new(NodeRenderer::Mul(vec![
                        NodeRenderer::Const(18.85),
                        NodeRenderer::Time,
                    ])),
                ),
            ]),
        ]);
        let back = Index::Neg(Box::new(Index::Step(Box::new(length), Round::Floor)));
        let echo = Index::Add(vec![now.clone(), Index::At(Map::whole(0, -1489))]);
        NodeRenderer::Add(vec![
            NodeRenderer::Indexed {
                slot: Slot::Read(BufId(0)),
                index: Index::Add(vec![now, back]),
                reach: None,
            },
            NodeRenderer::Mul(vec![
                NodeRenderer::Const(0.5),
                NodeRenderer::Indexed {
                    slot: Slot::Own,
                    index: echo,
                    reach: Some((-1489, -1489)),
                },
            ]),
        ])
    }

    /// Every index a sample reads, its delay's step included, is rounded in doubles or `i64`:
    /// none in 128-bit integers.
    #[test]
    fn a_delay_and_an_echo_round_no_index_in_wide_integers() {
        let to = 9_600;
        let layout = Layout {
            grid: Grid::of(48_000),
            width: 1,
            read_widths: vec![1],
            sites: Vec::new(),
        };
        let spanned =
            Spanned::new(&delayed(), &layout, (0, to), &[Extent::EVERYWHERE]).expect("a program");
        let mut input = Buffer::empty(48_000, 1, to as usize, 0);
        (0..to).for_each(|n| input.push(0, (n as f64 * 0.01).sin()));
        let mut out = Buffer::empty(48_000, 1, to as usize, 0);
        let mut machine = Machine::over(&spanned, 0).expect("a machine");
        let before = WIDE.with(std::cell::Cell::get);
        machine
            .run_to(to, &[input.within(Extent::from(0))], (&mut out, 0))
            .expect("samples");
        assert_eq!(WIDE.with(std::cell::Cell::get), before);
    }
}
