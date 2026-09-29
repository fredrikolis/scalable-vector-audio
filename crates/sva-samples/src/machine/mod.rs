// Concern: runs one node renderer a block at a time, span after span, over a tape | Non-concern: the op array's own shape (ops.rs), cutting the spans (live.rs) | IO: (Spanned, reads, tape) -> samples

mod block;
mod live;
pub mod ops;
mod read;
pub mod renderer;
pub mod tape;

use crate::error::SampleError;
use crate::filters::FilterSite;
use crate::physics::{Solver, site};
use block::{BLOCK, Block, Here};
use ops::{Layout, Op, lowered};
use renderer::{Formula, Grid, Index, NodeRenderer, Site, Slot};
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
        let mut pending = Vec::new();
        let mut args = Vec::with_capacity(lowered.ops.len());
        for (slot, op) in lowered.ops.iter().enumerate() {
            args.push(pending.split_off(pending.len() - arity_of(op)));
            pending.push(slot);
        }
        Ok(Program {
            ops: lowered.ops,
            widths: lowered.widths,
            args,
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
            let len = p.block(from, (to - from).min(BLOCK as i64) as usize);
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
    /// Up to `most` samples from `from`, each reading its own past only before `from`.
    fn block(&self, from: i64, most: usize) -> usize {
        let mut len = most;
        for op in &self.ops {
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
                    while len > 1 && at.at(from).max(at.at(last(len))) >= from {
                        len /= 2;
                    }
                }
                Op::Indexed {
                    slot: Slot::Own,
                    reach,
                    ..
                } => {
                    len = match reach {
                        Some((_, most)) if *most < 0 => len.min(most.unsigned_abs() as usize),
                        _ => 1,
                    }
                }
                _ => {}
            }
        }
        len.max(1)
    }
}

fn arity_of(op: &Op) -> usize {
    match op {
        Op::Const(_)
        | Op::Time
        | Op::Wrap(_)
        | Op::Noise(_)
        | Op::Read { .. }
        | Op::ReadScaled { .. } => 0,
        Op::Physics { arity, .. } => *arity,
        Op::Map(_) | Op::Crop { .. } | Op::Channel(_) | Op::Formula { .. } => 1,
        Op::Indexed { arity, .. } | Op::Instant { arity, .. } => *arity,
        Op::Sub | Op::Div | Op::Pow | Op::Zip(_) => 2,
        Op::Add(n) | Op::Mul(n) | Op::Join(n) => *n,
        Op::Filter { .. } => 4,
    }
}
