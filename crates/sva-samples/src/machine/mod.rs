// Concern: runs one node renderer a sample at a time, whole or span after span | Non-concern: the op array's own shape (ops.rs), the tree the engine hands over | IO: (NodeRenderer, Ctx) -> Buffer

mod live;
pub mod ops;
pub mod renderer;
pub mod tape;

use crate::buffer::Buffer;
use crate::error::SampleError;
use crate::filters::FilterSite;
use crate::physics::{Solver, site};
use ops::{Layout, Op, lower};
use renderer::{NodeRenderer, Site};
use tape::{Tape, Window};

pub use live::Span;

pub use ops::Layout as MachineLayout;

/// The op array, one width per slot, and the call sites the run opens state for.
#[derive(Clone)]
struct Program {
    ops: Vec<Op>,
    widths: Vec<usize>,
    sites: Vec<Site>,
    pub width: usize,
}

/// A whole run writes `len` samples from grid sample `start`, reading each slot's window.
pub struct Ctx<'a> {
    pub rate: u32,
    pub start: i64,
    pub len: usize,
    pub reads: &'a [Window<'a>],
}

impl NodeRenderer {
    fn compile(&self, layout: &Layout) -> Result<Program, SampleError> {
        let (mut ops, mut widths) = (Vec::new(), Vec::new());
        let width = lower(self, layout, &mut ops, &mut widths)?;
        Ok(Program {
            ops,
            widths,
            sites: layout.sites.clone(),
            width,
        })
    }

    /// A loop reads what this run wrote, silent before `start`.
    pub fn run(&self, layout: &Layout, ctx: &Ctx) -> Result<Buffer, SampleError> {
        let mut machine = Machine::open(self, layout, ctx.rate)?;
        let mut own = Tape::new(machine.width(), ctx.len, ctx.start);
        machine.steps(ctx.start + ctx.len as i64, ctx.reads, &mut own)?;
        let mut out = Buffer::of_planes(ctx.rate, own.into_planes());
        out.start = ctx.start;
        Ok(out)
    }
}

#[derive(Clone)]
enum State {
    Filter(FilterSite),
    Physics(Box<dyn Solver>, Heard),
}

/// A physics site's loudest sample in each chunk of `step` steps; `step` 0 keeps none.
#[derive(Clone, Debug, Default)]
pub struct Heard {
    step: u64,
    count: u64,
    open: f64,
    chunks: Vec<f64>,
}

impl Heard {
    pub(crate) fn every(step: u64) -> Heard {
        Heard {
            step,
            ..Heard::default()
        }
    }

    pub(crate) fn note(&mut self, v: f64) {
        if self.step == 0 {
            return;
        }
        self.open = self.open.max(v.abs());
        self.count += 1;
        if self.count.is_multiple_of(self.step) {
            self.chunks.push(std::mem::take(&mut self.open));
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn chunks(&self) -> &[f64] {
        &self.chunks
    }
}

impl Clone for Box<dyn Solver> {
    fn clone(&self) -> Box<dyn Solver> {
        self.boxed()
    }
}

/// A filter site carries one lane per component of its widest argument, which the compiled
/// slot width already states.
fn open(p: &Program, rate: u32) -> Result<Vec<State>, SampleError> {
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
                    f64::from(rate),
                )),
                Site::Physics(params) => State::Physics(site(params, rate)?, Heard::default()),
            })
        })
        .collect()
}

/// One value slot per op and a stack of the indices waiting. Nothing allocates in the loop.
#[derive(Clone)]
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
    v[c % v.len()]
}

/// One compiled node and every call site's state, run over any span of the grid in order.
/// A span continues exactly where the last ended, so blocks write the samples one run would.
#[derive(Clone)]
pub struct Machine {
    program: Program,
    states: Vec<State>,
    stack: Stack,
    rate: u32,
}

#[derive(Clone)]
pub struct MachineState {
    sites: Vec<Site>,
    states: Vec<State>,
}

impl Machine {
    pub fn open(
        renderer: &NodeRenderer,
        layout: &Layout,
        rate: u32,
    ) -> Result<Machine, SampleError> {
        let program = renderer.compile(layout)?;
        let states = open(&program, rate)?;
        let stack = Stack::of(&program.widths);
        Ok(Machine {
            program,
            states,
            stack,
            rate,
        })
    }

    pub fn width(&self) -> usize {
        self.program.width
    }

    pub fn run_to(&mut self, to: i64, reads: &[Window], own: &mut Tape) -> Result<(), SampleError> {
        for state in &mut self.states {
            if let State::Filter(filter) = state {
                filter.forget_frames();
            }
        }
        self.steps(to, reads, own)
    }

    fn steps(&mut self, to: i64, reads: &[Window], own: &mut Tape) -> Result<(), SampleError> {
        let sr = f64::from(self.rate);
        let p = &self.program;
        for n in own.end()..to {
            let here = Here {
                reads,
                own,
                n,
                t: n as f64 / sr,
                sr,
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
            State::Physics(solver, _) => Some(solver.as_ref()),
            State::Filter(_) => None,
        }
    }

    pub fn hear(&mut self, step: u64) {
        for state in &mut self.states {
            if let State::Physics(_, heard) = state {
                heard.step = step;
            }
        }
    }

    pub fn heard(&self, site: usize) -> Option<&Heard> {
        match self.states.get(site)? {
            State::Physics(_, heard) => Some(heard),
            State::Filter(_) => None,
        }
    }

    pub fn filter(&self, site: usize) -> Option<&FilterSite> {
        match self.states.get(site)? {
            State::Filter(filter) => Some(filter),
            State::Physics(..) => None,
        }
    }

    pub fn state(&self) -> MachineState {
        MachineState {
            sites: self.program.sites.clone(),
            states: self.states.clone(),
        }
    }

    /// Takes `held`'s state site by site. A solver whose release alone moved keeps its own
    /// parameters and takes only the motion; any other difference refuses.
    pub fn carry(&mut self, held: &MachineState) -> Result<(), SampleError> {
        if held.sites.len() != self.program.sites.len() {
            return Err(SampleError::StateMismatch);
        }
        for (at, (mine, theirs)) in self.program.sites.iter().zip(&held.sites).enumerate() {
            let taken = match (mine, theirs, &mut self.states[at], &held.states[at]) {
                _ if mine == theirs => {
                    self.states[at] = held.states[at].clone();
                    true
                }
                (
                    Site::Physics(a),
                    Site::Physics(b),
                    State::Physics(solver, heard),
                    State::Physics(motion, held),
                ) if a.differs_in_release_alone(b) => {
                    *heard = held.clone();
                    solver.take_motion(motion.as_ref())
                }
                _ => false,
            };
            if !taken {
                return Err(SampleError::StateMismatch);
            }
        }
        Ok(())
    }
}

struct Here<'a> {
    reads: &'a [Window<'a>],
    own: &'a Tape,
    n: i64,
    t: f64,
    sr: f64,
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
    for (slot, op) in p.ops.iter().enumerate() {
        let at = stack.pending.len() - arity_of(op);
        let (done, rest) = stack.values.split_at_mut(slot);
        fill(op, done, &stack.pending[at..], &mut rest[0], here, states)?;
        stack.pending.truncate(at);
        stack.pending.push(slot);
    }
    Ok(())
}

fn arity_of(op: &Op) -> usize {
    match op {
        Op::Const(_) | Op::Time | Op::Read { .. } | Op::SelfAt { .. } | Op::Physics { .. } => 0,
        Op::Map(_) | Op::Crop { .. } | Op::Channel(_) => 1,
        Op::Sub | Op::Div | Op::Pow | Op::Zip(_) => 2,
        Op::Add(n) | Op::Mul(n) | Op::Join(n) => *n,
        Op::Filter { .. } => 4,
    }
}

fn fill(
    op: &Op,
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
        Op::Read { id, shift } => {
            let window = here.reads[id.0 as usize];
            let at = n + shift;
            for (c, slot) in result.iter_mut().enumerate() {
                *slot = window.at(c, at);
            }
        }
        Op::SelfAt { steps } => {
            let at = n - i64::from(*steps);
            for (c, slot) in result.iter_mut().enumerate() {
                *slot = here.own.window().at(c, at);
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
        Op::Crop { a, b, rise, fall } => {
            let gain = crate::collapse::crop_gain(t, *a, *b, *rise, *fall);
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
            let State::Physics(solver, heard) = &mut states[site.0 as usize] else {
                unreachable!("a physics op names a physics site")
            };
            result[0] = solver.step()?;
            heard.note(result[0]);
        }
    }
    Ok(())
}
