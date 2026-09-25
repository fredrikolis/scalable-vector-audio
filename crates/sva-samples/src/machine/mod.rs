// Concern: runs one node renderer a sample at a time, whole or span after span | Non-concern: the op array's own shape (ops.rs), the tree the engine hands over | IO: (NodeRenderer, Ctx) -> Buffer

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

pub use ops::Layout as MachineLayout;

/// The op array, one width per slot, and the call sites the run opens state for.
struct Program {
    ops: Vec<Op>,
    widths: Vec<usize>,
    sites: Vec<Site>,
    pub width: usize,
}

/// `self_planes` is this node's own output so far, planar: component `c` owns
/// `[c*len, (c+1)*len)`, and only the first `written` samples are founded. `run` refounds
/// that mark every sample, so no block sizes the loop.
pub struct Ctx<'a> {
    pub rate: u32,
    pub origin_secs: f64,
    pub len: usize,
    pub reads: &'a [Window<'a>],
    pub self_planes: &'a [f64],
    pub written: usize,
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

    /// A loop reads what this run wrote; a caller's own `self_planes` replaces it.
    pub fn run(&self, layout: &Layout, ctx: &Ctx) -> Result<Buffer, SampleError> {
        let mut machine = Machine::open(self, layout, ctx.rate, ctx.origin_secs)?;
        let mut own = Tape::new(machine.width(), ctx.len);
        let past = match ctx.self_planes.is_empty() {
            true => Past::Own,
            false => Past::Fixed {
                planes: ctx.self_planes,
                len: ctx.len,
                written: ctx.written,
            },
        };
        machine.steps(ctx.len, ctx.reads, &past, &mut own)?;
        let mut out = Buffer::of_planes(ctx.rate, own.into_planes());
        out.origin_secs = ctx.origin_secs;
        Ok(out)
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
fn open(p: &Program, rate: u32) -> Result<Vec<State>, SampleError> {
    let mut lanes = vec![1usize; p.sites.len()];
    for (slot, op) in p.ops.iter().enumerate() {
        if let Op::Filter(id) = op {
            lanes[id.0 as usize] = p.widths[slot];
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
                Site::Physics(params) => State::Physics(site(params, rate)?),
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
    v[c % v.len()]
}

enum Past<'a> {
    Own,
    Fixed {
        planes: &'a [f64],
        len: usize,
        written: usize,
    },
}

/// One compiled node and every call site's state, run over any span of the grid in order.
/// A span continues exactly where the last ended, so blocks write the samples one run would.
pub struct Machine {
    program: Program,
    states: Vec<State>,
    stack: Stack,
    rate: u32,
    origin_secs: f64,
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
        origin_secs: f64,
    ) -> Result<Machine, SampleError> {
        let program = renderer.compile(layout)?;
        let states = open(&program, rate)?;
        let stack = Stack::of(&program.widths);
        Ok(Machine {
            program,
            states,
            stack,
            rate,
            origin_secs,
        })
    }

    pub fn width(&self) -> usize {
        self.program.width
    }

    pub fn run_to(
        &mut self,
        to: usize,
        reads: &[Window],
        own: &mut Tape,
    ) -> Result<(), SampleError> {
        for state in &mut self.states {
            if let State::Filter(filter) = state {
                filter.forget_frames();
            }
        }
        self.steps(to, reads, &Past::Own, own)
    }

    fn steps(
        &mut self,
        to: usize,
        reads: &[Window],
        past: &Past,
        own: &mut Tape,
    ) -> Result<(), SampleError> {
        let sr = f64::from(self.rate);
        let p = &self.program;
        for i in own.end()..to {
            let t = self.origin_secs + i as f64 / sr;
            let here = Here {
                reads,
                past,
                own,
                i,
                t,
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
                    State::Physics(solver),
                    State::Physics(motion),
                ) if a.differs_in_release_alone(b) => solver.take_motion(motion.as_ref()),
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
    past: &'a Past<'a>,
    own: &'a Tape,
    i: usize,
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
        Op::Const(_) | Op::Time | Op::Read { .. } | Op::SelfAt { .. } | Op::Physics(_) => 0,
        Op::Map(_) | Op::Crop { .. } | Op::Channel(_) => 1,
        Op::Sub | Op::Div | Op::Pow | Op::Zip(_) => 2,
        Op::Add(n) | Op::Mul(n) | Op::Join(n) => *n,
        Op::Filter(_) => 4,
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
    let (i, t, sr) = (here.i, here.t, here.sr);
    let arg = |k: usize| done[srcs[k]].as_slice();
    match op {
        Op::Const(v) => result[0] = *v,
        Op::Time => result[0] = t,
        Op::Read { id, shift } => {
            let window = here.reads[id.0 as usize];
            let at = i as i64 + shift;
            for (c, slot) in result.iter_mut().enumerate() {
                *slot = window.at(c, at);
            }
        }
        Op::SelfAt { steps } => {
            let at = i as i64 - i64::from(*steps);
            for (c, slot) in result.iter_mut().enumerate() {
                *slot = match here.past {
                    Past::Own => here.own.window().at(c, at),
                    Past::Fixed {
                        planes,
                        len,
                        written,
                    } => usize::try_from(at)
                        .ok()
                        .filter(|k| k < written)
                        .and_then(|k| planes.get(c * len + k).copied())
                        .unwrap_or(0.0),
                };
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
        Op::Filter(id) => {
            let State::Filter(filter) = &mut states[id.0 as usize] else {
                unreachable!("a filter op names a filter site")
            };
            filter.process(arg(0), arg(1), arg(2), arg(3), result, sr, i);
        }
        Op::Physics(id) => {
            let State::Physics(solver) = &mut states[id.0 as usize] else {
                unreachable!("a physics op names a physics site")
            };
            result[0] = solver.step()?;
        }
    }
    Ok(())
}
