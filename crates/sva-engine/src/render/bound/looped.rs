// Concern: bounds a loop `y = F + sum g y(t - d)` from each grid instant on | Non-concern: a loop across nodes, refused where bounds are built | IO: (F's bounds, taps, instant) -> a bound

use sva_formula::NodeId;

use super::envelope::{STEP, Unbounded, constant, holds_self};
use super::range::OP;
use crate::loops::Delay;
use crate::typing::{Typing, Value};

/// The roundings one step of the loop makes, each `OP` of what it combines.
const OPS_PER_STEP: f64 = 64.0;

/// A sum of nodes' bounds, each scaled, summed in the order the loop body writes them.
#[derive(Clone, Debug)]
pub(crate) enum Lin {
    Node(NodeId),
    Sum(Box<Lin>, Box<Lin>),
    Scaled(Box<Lin>, f64),
}

impl Lin {
    fn eval(&self, read: &dyn Fn(NodeId) -> f64) -> f64 {
        match self {
            Lin::Node(id) => read(*id),
            Lin::Sum(a, b) => a.eval(read) + b.eval(read),
            Lin::Scaled(a, k) => a.eval(read) * k,
        }
    }
}

/// `|body| <= |free| + sum g |y(n - d)|`.
#[derive(Default)]
struct Affine {
    free: Option<Lin>,
    taps: Vec<(f64, usize)>,
}

impl Affine {
    fn add(mut self, other: Affine) -> Affine {
        self.free = match (self.free, other.free) {
            (Some(a), Some(b)) => Some(Lin::Sum(Box::new(a), Box::new(b))),
            (a, b) => a.or(b),
        };
        self.taps.extend(other.taps);
        self
    }

    fn scaled(mut self, k: f64) -> Affine {
        self.free = self.free.map(|e| Lin::Scaled(Box::new(e), k));
        self.taps.iter_mut().for_each(|(g, _)| *g *= k);
        self
    }

    fn peak(&self, peak: &dyn Fn(NodeId) -> f64) -> f64 {
        self.free.as_ref().map_or(0.0, |f| f.eval(peak))
    }
}

/// `|y(n)| <= |F(n)| + sum g_i |y(n - D_i)|` with `sum g_i < 1`, iterated on the grid.
/// One tap is unrolled across a whole step, so a short delay decays per delay rather than
/// per step.
pub(crate) struct Looped {
    owner: NodeId,
    leaves: Vec<NodeId>,
    free: Option<Lin>,
    taps: Vec<(f64, usize)>,
    largest: f64,
    slack: f64,
    own: Vec<f64>,
}

type Found<'r> = &'r dyn Fn(NodeId) -> Result<f64, Unbounded>;

impl Looped {
    pub(crate) fn pending(owner: NodeId) -> Looped {
        Looped {
            owner,
            leaves: Vec::new(),
            free: None,
            taps: Vec::new(),
            largest: 0.0,
            slack: 0.0,
            own: Vec::new(),
        }
    }

    /// Every node the body reads outside the loop, each to instant `j`.
    pub(super) fn inputs(&self, tys: &Typing, j: usize) -> Vec<(NodeId, usize)> {
        let mut leaves = Vec::new();
        leaves_of(tys, self.owner, &mut leaves);
        leaves.into_iter().map(|n| (n, j + 1)).collect()
    }

    pub(super) fn start(&mut self, tys: &Typing, found: Found, rate: f64) -> Result<(), Unbounded> {
        let refuse = |what: &str| Unbounded {
            node: tys.name(self.owner).to_string(),
            class: what.to_string(),
        };
        let form = affine(tys, self.owner, found, rate)?.map_err(|what| refuse(&what))?;
        let gain: f64 = form.taps.iter().map(|(g, _)| g).sum();
        if gain >= 1.0 {
            return Err(refuse("a loop whose gain does not contract"));
        }
        let peak = |n: NodeId| found(n).unwrap_or(f64::INFINITY);
        self.largest = form.peak(&peak) / (1.0 - gain);
        self.slack = OP * OPS_PER_STEP * (1.0 + gain) * self.largest / (1.0 - gain);
        self.free = form.free;
        self.taps = form.taps;
        leaves_of(tys, self.owner, &mut self.leaves);
        Ok(())
    }

    pub(super) fn before(&self) -> f64 {
        self.largest + self.slack
    }

    /// `read(node, i)` is a node's bound from instant `i`, each through instant `j`.
    pub(super) fn at(&mut self, read: &dyn Fn(NodeId, usize) -> f64, peak: Found, j: usize) -> f64 {
        let free_before = self
            .free
            .as_ref()
            .map_or(0.0, |f| f.eval(&|n| peak(n).unwrap_or(f64::INFINITY)));
        let free_at = |i: usize| self.free.as_ref().map_or(0.0, |f| f.eval(&|n| read(n, i)));
        let index = |sample: i64| (sample >= 0).then_some(sample as usize / STEP);
        let free = |sample: i64| free_at(index(sample).unwrap_or(0));
        let own = |sample: i64| index(sample).map_or(self.own[0], |i| self.own[i]);
        let held = match j {
            0 => self.largest,
            _ => {
                let n = (j * STEP) as i64;
                let next = match self.taps.as_slice() {
                    [(g, d)] => {
                        let times = (STEP / *d).max(1);
                        let mut held = 0.0;
                        let mut weight = 1.0;
                        for k in 0..times {
                            held += weight * free(n - (k * d) as i64).min(free_before);
                            weight *= g;
                        }
                        held + weight * own(n - (times * d) as i64)
                    }
                    taps => {
                        free_at(j)
                            + taps
                                .iter()
                                .map(|(g, d)| g * own(n - *d as i64))
                                .sum::<f64>()
                    }
                };
                next.min(self.own[j - 1])
            }
        };
        self.own.push(held);
        held + self.slack
    }
}

/// The nodes a loop body reads that do not hold the loop, in the order it reads them.
fn leaves_of(tys: &Typing, id: NodeId, out: &mut Vec<NodeId>) {
    if !holds_self(tys, id) {
        if !out.contains(&id) {
            out.push(id);
        }
        return;
    }
    if let Value::Op { args, .. } = tys.value(id) {
        for arg in args {
            leaves_of(tys, *arg, out);
        }
    }
}

/// The loop body as a bound linear in its own delayed output. A map with `|f(u)| <= |u|`
/// keeps a bound of that shape, and so does a product with a signal bounded everywhere.
fn affine(
    tys: &Typing,
    id: NodeId,
    found: Found,
    rate: f64,
) -> Result<Result<Affine, String>, Unbounded> {
    if !holds_self(tys, id) {
        found(id)?;
        return Ok(Ok(Affine {
            free: Some(Lin::Node(id)),
            taps: Vec::new(),
        }));
    }
    let refuse = |what: &str| Ok(Err(what.to_string()));
    match tys.value(id).clone() {
        Value::SelfAt(delay) => match steps(delay, rate) {
            Some(d) => Ok(Ok(Affine {
                free: None,
                taps: vec![(1.0, d)],
            })),
            None => refuse("a loop whose delay moves"),
        },
        Value::Op { name, args } => {
            let mut forms = Vec::with_capacity(args.len());
            for arg in &args {
                match affine(tys, *arg, found, rate)? {
                    Ok(f) => forms.push(f),
                    Err(e) => return Ok(Err(e)),
                }
            }
            let peak = |n: NodeId| found(n).unwrap_or(f64::INFINITY);
            match name.as_str() {
                "+" | "-" => Ok(Ok(forms.into_iter().fold(Affine::default(), Affine::add))),
                "*" | "/" => {
                    let looped = forms.iter().filter(|f| !f.taps.is_empty()).count();
                    if looped != 1 {
                        return refuse("a loop multiplied by itself");
                    }
                    let mut out = Affine::default();
                    let mut scale = 1.0;
                    for (at, form) in forms.into_iter().enumerate() {
                        match (form.taps.is_empty(), name.as_str(), at) {
                            (false, _, _) => out = form,
                            (true, "*", _) => scale *= form.peak(&peak),
                            (true, _, 1) => match constant(tys, args[1]) {
                                Some(d) if d != 0.0 => scale /= d.abs(),
                                _ => return refuse("a loop divided by a signal"),
                            },
                            (true, _, _) => return refuse("a loop under a division"),
                        }
                    }
                    Ok(Ok(out.scaled(scale)))
                }
                "crop" | "tanh" | "sat" | "sin" | "abs" => {
                    Ok(Ok(forms.into_iter().next().expect("an operand")))
                }
                _ => refuse("a loop through a map with no contraction"),
            }
        }
        _ => refuse("a loop through a filter or a transform"),
    }
}

fn steps(delay: Delay, rate: f64) -> Option<usize> {
    match delay {
        Delay::Steps(steps) => Some(steps as usize),
        Delay::Secs(secs) => Some(((secs * rate).round() as usize).max(1)),
        Delay::Varying => None,
    }
}
