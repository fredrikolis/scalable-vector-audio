// Concern: runs one node renderer a sample at a time, span after span, over a tape | Non-concern: the op array's own shape (ops.rs), cutting the spans (live.rs) | IO: (Spanned, reads, tape) -> samples

mod live;
pub mod ops;
mod read;
pub mod renderer;
pub mod tape;

use crate::error::SampleError;
use crate::filters::FilterSite;
use crate::physics::{Solver, site};
use ops::{Layout, Op, lowered};
use renderer::{Formula, Grid, Index, NodeRenderer, Site};
use tape::{Tape, Window};

pub use live::{Span, Spanned};

pub use ops::Layout as MachineLayout;

/// The op array, one width per slot, the formulas its ops name and the call sites the run
/// opens state for.
#[derive(Clone)]
pub(super) struct Program {
    ops: Vec<Op>,
    widths: Vec<usize>,
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

/// One value slot per op and a stack of the indices waiting. Nothing allocates in the loop.
struct Stack {
    values: Vec<Vec<f64>>,
    pending: Vec<usize>,
}

impl Stack {
    fn of(widths: &[usize]) -> Stack {
        Stack {
            values: widths.iter().map(|&w| vec![0.0; w]).collect(),
            pending: Vec::with_capacity(widths.len()),
        }
    }
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
    stack: Stack,
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
    /// What one copy of this state holds.
    pub fn bytes(&self) -> usize {
        let states: usize = self.states.iter().map(State::bytes).sum();
        size_of::<Self>() + std::mem::size_of_val(self.sites.as_slice()) + states
    }
}

impl Machine {
    /// Stepping from `at`, each span's own program from where it starts.
    pub fn over(spanned: &Spanned, grid: Grid, at: i64) -> Result<Machine, SampleError> {
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
        let stack = Stack::of(&first.widths);
        Ok(Machine {
            program: first,
            states,
            stack,
            grid,
            ahead,
        })
    }

    pub fn width(&self) -> usize {
        self.program.width
    }

    /// Whether any call site holds state of its own.
    pub fn stateful(&self) -> bool {
        !self.program.sites.is_empty()
    }

    /// What its call sites hold.
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
                self.stack = Stack::of(&program.widths);
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
        let (grid, sr) = (self.grid, self.grid.sr());
        let p = &self.program;
        for n in own.end()..to {
            let here = Here {
                reads,
                own,
                n,
                t: grid.instant(n),
                sr,
                grid,
            };
            step(p, &here, &mut self.states, &mut self.stack)?;
            let top = &self.stack.values[*self
                .stack
                .pending
                .last()
                .expect("a renderer leaves one value")];
            for c in 0..p.width {
                own.push(c, part(top, c));
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

struct Here<'a> {
    reads: &'a [Window<'a>],
    own: &'a Tape,
    n: i64,
    t: f64,
    sr: f64,
    grid: Grid,
}

impl Here<'_> {
    fn source(&self, slot: renderer::Slot) -> read::Source<'_> {
        match slot {
            renderer::Slot::Read(id) => read::Source {
                window: self.reads[id.0 as usize],
                limit: None,
            },
            renderer::Slot::Own => read::Source {
                window: self.own.window(),
                limit: Some(self.n),
            },
        }
    }
}

/// One sample of the whole renderer. Postfix order puts every operand's slot before the slot
/// that consumes it, so `split_at_mut` hands out the reads and the one write at once.
fn step(
    p: &Program,
    here: &Here,
    states: &mut [State],
    stack: &mut Stack,
) -> Result<(), SampleError> {
    stack.pending.clear();
    let mut slot = 0;
    while slot < p.ops.len() {
        let op = &p.ops[slot];
        let at = stack.pending.len() - arity_of(op);
        let (done, rest) = stack.values.split_at_mut(slot);
        fill(
            (op, p),
            done,
            &stack.pending[at..],
            &mut rest[0],
            here,
            states,
        )?;
        stack.pending.truncate(at);
        stack.pending.push(slot);
        slot += 1;
    }
    Ok(())
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

fn fill(
    (op, p): (&Op, &Program),
    done: &[Vec<f64>],
    srcs: &[usize],
    result: &mut [f64],
    here: &Here,
    states: &mut [State],
) -> Result<(), SampleError> {
    let (n, t, sr) = (here.n, here.t, here.sr);
    let arg = |k: usize| done[srcs[k]].as_slice();
    match op {
        Op::Const(v) => result[0] = *v,
        Op::Time => result[0] = t,
        Op::Wrap(wrap) => {
            result[0] = wrap
                .at(n, here.grid)
                .ok_or(SampleError::UnreadablePosition)?
        }
        Op::Noise(seed) => result[0] = sva_formula::draw(*seed, here.grid.position(n)),
        Op::Indexed {
            slot, at, reach, ..
        } => {
            let k = p.indices[*at]
                .at(n, here.grid, &|j| arg(*j)[0])
                .ok_or(SampleError::UnreadablePosition)?;
            if let Some((least, most)) = reach
                && !(*least..=*most).contains(&k.saturating_sub(n))
            {
                return Err(SampleError::ReadsAhead { at: k });
            }
            here.source(*slot).nearest(k, result)?;
        }
        Op::Instant { at, .. } => {
            let k = p.indices[*at]
                .at(n, here.grid, &|j| arg(*j)[0])
                .ok_or(SampleError::UnreadablePosition)?;
            result[0] = here.grid.instant(k);
        }
        Op::Read { slot, at } => here.source(*slot).mapped(*at, n, result)?,
        Op::ReadScaled { slot, at, by } => {
            here.source(*slot).mapped(*at, n, result)?;
            for v in result.iter_mut() {
                *v *= by;
            }
        }
        Op::Formula { at } => {
            for (c, slot) in result.iter_mut().enumerate() {
                *slot = p.formulas[*at]
                    .at(c, part(arg(0), c))
                    .map_err(|_| SampleError::FormulaUnevaluable { at: n })?;
            }
        }
        Op::Add(_) | Op::Mul(_) => {
            let product = matches!(op, Op::Mul(_));
            for (c, slot) in result.iter_mut().enumerate() {
                *slot =
                    srcs.iter()
                        .enumerate()
                        .fold(f64::from(u8::from(product)), |acc, (k, _)| {
                            if product {
                                acc * part(arg(k), c)
                            } else {
                                acc + part(arg(k), c)
                            }
                        });
            }
        }
        Op::Sub | Op::Div | Op::Pow | Op::Zip(_) => {
            for (c, slot) in result.iter_mut().enumerate() {
                let (a, b) = (part(arg(0), c), part(arg(1), c));
                *slot = match op {
                    Op::Sub => a - b,
                    Op::Div => a / b,
                    Op::Pow => a.powf(b),
                    Op::Zip(f) => f.apply(a, b),
                    _ => unreachable!("the arm's own guard"),
                };
            }
        }
        Op::Map(f) => {
            for (c, slot) in result.iter_mut().enumerate() {
                *slot = f.apply(part(arg(0), c));
            }
        }
        Op::Crop {
            window,
            a,
            b,
            rise,
            fall,
        } => {
            let gain = match window.0 <= n && n < window.1 {
                true => crate::collapse::shoulders(t, *a, *b, *rise, *fall),
                false => 0.0,
            };
            for (c, slot) in result.iter_mut().enumerate() {
                *slot = match gain {
                    0.0 => 0.0,
                    gain => part(arg(0), c) * gain,
                };
            }
        }
        Op::Join(_) => {
            let mut c = 0;
            for k in 0..srcs.len() {
                for &v in arg(k) {
                    result[c] = v;
                    c += 1;
                }
            }
        }
        Op::Channel(k) => result[0] = arg(0)[*k],
        Op::Filter { from, .. } | Op::Physics { from, .. } if n < *from => result.fill(0.0),
        Op::Filter { site, .. } => {
            let State::Filter(filter) = &mut states[site.0 as usize] else {
                unreachable!("a filter op names a filter site")
            };
            filter.process(arg(0), arg(1), arg(2), arg(3), result, sr, n);
        }
        Op::Physics { site, .. } => {
            let State::Physics(solver) = &mut states[site.0 as usize] else {
                unreachable!("a physics op names a physics site")
            };
            let mut args = [0.0; crate::physics::MAX_VARYING];
            for (k, slot) in args.iter_mut().enumerate().take(srcs.len()) {
                *slot = arg(k)[0] + 0.0;
            }
            result[0] = solver.step(&args[..srcs.len()])?;
        }
    }
    Ok(())
}
