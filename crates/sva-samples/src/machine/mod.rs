// Concern: runs one node renderer a block at a time, span after span, over a tape | Non-concern: the op array's own shape (ops.rs), cutting the spans (live.rs) | IO: (Spanned, reads, tape) -> samples

mod block;
mod live;
pub mod ops;
mod read;
pub mod renderer;
pub mod tape;

use crate::error::SampleError;
use crate::filters::FilterSite;
use crate::grid::Grid;
use crate::physics::{Solver, site};
use block::{BLOCK, Block, Here};
use ops::{Layout, Op, lowered};
use renderer::{Formula, Index, NodeRenderer, Site, Slot};
use tape::{Tape, Window};

pub use live::{Span, Spanned};

pub use ops::Layout as MachineLayout;

/// The op array, each op's width and operand slots, the formulas its ops name and the call
/// sites the run opens state for.
#[derive(Clone)]
pub(super) struct Program {
    ops: Vec<Op>,
    widths: Vec<usize>,
    args: Vec<Vec<usize>>,
    formulas: Vec<Formula>,
    indices: Vec<Index<usize>>,
    sites: Vec<Site>,
    pub width: usize,
}

impl NodeRenderer {
    pub(super) fn compile(&self, layout: &Layout) -> Result<Program, SampleError> {
        let (lowered, width) = lowered(self, layout)?;
        Ok(Program {
            ops: lowered.ops,
            widths: lowered.widths,
            args: lowered.args,
            formulas: lowered.formulas,
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
fn open(p: &Program, grid: Grid) -> Result<Vec<State>, SampleError> {
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

/// One compiled node and every call site's state, run over any span of the grid in order.
/// A span continues exactly where the last ended, so blocks write the samples one run would.
pub struct Machine {
    program: Program,
    states: Vec<State>,
    block: Block,
    grid: Grid,
    /// Each later span's first sample and the program it runs, the next one last.
    ahead: Vec<(i64, Program)>,
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
        let mut ahead: Vec<(i64, Program)> = spanned
            .compiled()
            .iter()
            .filter(|(span, _)| span.to > at)
            .map(|(span, program)| (span.from, program.clone()))
            .collect();
        ahead.reverse();
        let first = match ahead.pop() {
            Some((_, program)) => program,
            None => spanned.silent()?,
        };
        let states = open(&first, grid)?;
        let block = Block::of(&first.widths);
        Ok(Machine {
            program: first,
            states,
            block,
            grid,
            ahead,
        })
    }

    pub fn width(&self) -> usize {
        self.program.width
    }

    pub fn stateful(&self) -> bool {
        !self.program.sites.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.states.iter().map(State::bytes).sum()
    }

    pub fn run_to(&mut self, to: i64, reads: &[Window], own: &mut Tape) -> Result<(), SampleError> {
        for state in &mut self.states {
            if let State::Filter(filter) = state {
                filter.forget_frames();
            }
        }
        self.steps(to, reads, own)
    }

    /// `run_to`, the filters' frames kept.
    pub fn run_on(&mut self, to: i64, reads: &[Window], own: &mut Tape) -> Result<(), SampleError> {
        self.steps(to, reads, own)
    }

    fn steps(&mut self, to: i64, reads: &[Window], own: &mut Tape) -> Result<(), SampleError> {
        loop {
            while let Some((from, _)) = self.ahead.last()
                && *from <= own.end()
            {
                let (_, program) = self.ahead.pop().expect("a span ahead");
                self.block = Block::of(&program.widths);
                self.program = program;
            }
            let until = self.ahead.last().map_or(to, |(from, _)| (*from).min(to));
            self.stepped(until, reads, own)?;
            if own.end() >= to {
                return Ok(());
            }
        }
    }

    fn stepped(&mut self, to: i64, reads: &[Window], own: &mut Tape) -> Result<(), SampleError> {
        let p = &self.program;
        while own.end() < to {
            let from = own.end();
            let most = (to - from).min(BLOCK as i64) as usize;
            let len = p.block(from, most, &mut self.block.recurrent);
            let here = Here {
                reads,
                own: own.window(),
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
            sites: self.program.sites.clone(),
            states: self.states.clone(),
        }
    }

    pub fn restart(&mut self) {
        self.states = open(&self.program, self.grid).expect("the sites opened once already");
    }

    /// Whether `carry` takes `held`: the same sites, a varying parameter's values aside.
    pub fn accepts(&self, held: &MachineState) -> bool {
        held.sites == self.program.sites
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

impl Program {
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

/// Passes over a block, ops run over part of one, and samples read one by one, per thread.
#[cfg(test)]
mod counts {
    use std::cell::Cell;

    thread_local! {
        pub(super) static PASSES: Cell<u64> = const { Cell::new(0) };
        pub(super) static FILLS: Cell<u64> = const { Cell::new(0) };
        pub(super) static READS: Cell<u64> = const { Cell::new(0) };
    }
}

#[cfg(test)]
mod tests {
    use super::counts::{FILLS, PASSES, READS};
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

    fn ran(to: i64, step: i64) -> (Vec<Vec<f64>>, [u64; 3]) {
        let layout = Layout {
            grid: Grid::of(48_000),
            width: 2,
            read_widths: vec![1],
            sites: Vec::new(),
        };
        let spanned =
            Spanned::new(&feedback(), &layout, (0, to), &[Extent::EVERYWHERE]).expect("a program");
        let mut input = Tape::new(1, to as usize, 0);
        (0..to).for_each(|n| input.push(0, f64::from(u8::from(n % 4800 == 0))));
        let mut out = Tape::new(2, to as usize, 0);
        let mut machine = Machine::over(&spanned, 0).expect("a machine");
        let counted = || [&PASSES, &FILLS, &READS].map(|c| c.with(std::cell::Cell::get));
        let before = counted();
        let mut at = 0;
        while at < to {
            at = (at + step).min(to);
            machine
                .run_to(at, &[input.window()], &mut out)
                .expect("samples");
        }
        let after = counted();
        (out.into_planes(), [0, 1, 2].map(|k| after[k] - before[k]))
    }

    /// A pass per block, the far reads copied whole and only the damping run sample by sample,
    /// writing what a sample-at-a-time run does.
    #[test]
    fn a_loop_reading_itself_one_sample_back_runs_a_pass_per_block() {
        let to = 24_000;
        let (whole, [passes, fills, reads]) = ran(to, to);
        let (single, [one_by_one, ..]) = ran(to, 1);
        assert_eq!(whole, single);
        assert_eq!(one_by_one, to as u64);
        let blocks = (to as u64).div_ceil(BLOCK as u64);
        assert!(passes <= blocks + 1, "{passes} passes over {blocks} blocks");
        let per_sample = 2 * 4 + 1;
        let ops = feedback().ops(&Layout {
            grid: Grid::of(48_000),
            width: 2,
            read_widths: vec![1],
            sites: Vec::new(),
        });
        let ops = ops.expect("ops") as u64;
        assert!(
            fills <= passes * ops + per_sample * to as u64,
            "{fills} op runs"
        );
        let far = 2 * 2 * LONG.iter().sum::<i64>() as u64;
        assert!(reads <= 2 * 2 * to as u64 + far, "{reads} reads");
    }

    /// Each line reads both long lines and its own last sample, and both mix the same input:
    /// every identical read and mix runs once, writing the bits the loop's own arithmetic does.
    #[test]
    fn identical_reads_of_a_loops_own_past_run_once() {
        let to = 24_000;
        let layout = Layout {
            grid: Grid::of(48_000),
            width: 2,
            read_widths: vec![1],
            sites: Vec::new(),
        };
        assert_eq!(feedback().ops(&layout).expect("ops"), 20);
        let (planes, [_, fills, reads]) = ran(to, to);
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
        let blocks = (to as u64).div_ceil(BLOCK as u64) + 1;
        let (last, channels, scaled, sums, join) = (1, 2, 2, 2, 1);
        let per_sample = last + channels + scaled + sums + join;
        assert!(
            fills <= blocks * 20 + per_sample * to as u64,
            "{fills} op runs"
        );
        let far = 2 * 2 * LONG.iter().sum::<i64>() as u64;
        assert!(reads <= 2 * to as u64 + far, "{reads} reads");
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
        let mut input = Tape::new(1, to as usize, 0);
        (0..to).for_each(|n| input.push(0, (n as f64 * 0.01).sin()));
        let mut out = Tape::new(1, to as usize, 0);
        let mut machine = Machine::over(&spanned, 0).expect("a machine");
        let before = WIDE.with(std::cell::Cell::get);
        machine
            .run_to(to, &[input.window()], &mut out)
            .expect("samples");
        assert_eq!(WIDE.with(std::cell::Cell::get), before);
    }
}
