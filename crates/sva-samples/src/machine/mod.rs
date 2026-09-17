// Concern: runs one node renderer a sample at a time into a buffer | Non-concern: the op array's own shape (ops.rs), the tree the engine hands over (renderer.rs) | IO: (NodeRenderer, Ctx) -> Buffer

pub mod ops;
pub mod renderer;

use crate::buffer::Buffer;
use crate::error::SampleError;
use crate::filters::FilterSite;
use crate::physics::{Solver, site};
use ops::{Layout, Op, lower};
use renderer::{NodeRenderer, Site};

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
    pub reads: &'a [&'a Buffer],
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
        run(&self.compile(layout)?, ctx)
    }
}

enum State {
    Filter(FilterSite),
    Physics(Box<dyn Solver>),
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

fn run(p: &Program, ctx: &Ctx) -> Result<Buffer, SampleError> {
    let mut states = open(p, ctx.rate)?;
    let mut out = Buffer::silence(ctx.rate, p.width, ctx.len);
    out.origin_secs = ctx.origin_secs;
    let mut stack = Stack::of(&p.widths);
    let sr = f64::from(ctx.rate);
    let mut own = vec![0.0; p.width * ctx.len];
    for i in 0..ctx.len {
        {
            let held = Ctx {
                rate: ctx.rate,
                origin_secs: ctx.origin_secs,
                len: ctx.len,
                reads: ctx.reads,
                self_planes: match ctx.self_planes.is_empty() {
                    true => &own,
                    false => ctx.self_planes,
                },
                written: match ctx.self_planes.is_empty() {
                    true => i,
                    false => ctx.written,
                },
            };
            step(p, &held, &mut states, &mut stack, i, sr);
        }
        let top = &stack.values[*stack.pending.last().expect("a renderer leaves one value")];
        for c in 0..p.width {
            let value = part(top, c);
            out.planes[c][i] = value;
            own[c * ctx.len + i] = value;
        }
    }
    Ok(out)
}

/// One sample of the whole renderer. Postfix order puts every operand's slot before the slot
/// that consumes it, so `split_at_mut` hands out the reads and the one write at once.
fn step(p: &Program, ctx: &Ctx, states: &mut [State], stack: &mut Stack, i: usize, sr: f64) {
    let t = ctx.origin_secs + i as f64 / sr;
    stack.pending.clear();
    for (slot, op) in p.ops.iter().enumerate() {
        let at = stack.pending.len() - arity_of(op);
        let (done, rest) = stack.values.split_at_mut(slot);
        fill(
            op,
            done,
            &stack.pending[at..],
            &mut rest[0],
            ctx,
            states,
            i,
            t,
            sr,
        );
        stack.pending.truncate(at);
        stack.pending.push(slot);
    }
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

#[allow(clippy::too_many_arguments)]
fn fill(
    op: &Op,
    done: &[Vec<f64>],
    srcs: &[usize],
    result: &mut [f64],
    ctx: &Ctx,
    states: &mut [State],
    i: usize,
    t: f64,
    sr: f64,
) {
    let arg = |k: usize| done[srcs[k]].as_slice();
    match op {
        Op::Const(v) => result[0] = *v,
        Op::Time => result[0] = t,
        Op::Read { id, shift } => {
            let buffer = ctx.reads[id.0 as usize];
            let at = i as i64 + shift;
            for (c, slot) in result.iter_mut().enumerate() {
                *slot = usize::try_from(at)
                    .ok()
                    .and_then(|k| buffer.plane(c).get(k).copied())
                    .unwrap_or(0.0);
            }
        }
        Op::SelfAt { steps } => {
            let at = i as i64 - i64::from(*steps);
            for (c, slot) in result.iter_mut().enumerate() {
                *slot = usize::try_from(at)
                    .ok()
                    .filter(|k| *k < ctx.written)
                    .and_then(|k| ctx.self_planes.get(c * ctx.len + k).copied())
                    .unwrap_or(0.0);
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
        Op::Crop { a, b } => {
            let inside = t >= *a && t < *b;
            for (c, slot) in result.iter_mut().enumerate() {
                *slot = if inside { part(arg(0), c) } else { 0.0 };
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
            result[0] = solver.step();
        }
    }
}
