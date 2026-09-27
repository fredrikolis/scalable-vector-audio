// Concern: bounds each node's magnitude from every grid instant on, per node class | Non-concern: where a node is cut by the bound (cut/) | IO: (NodeId) -> Envelope, or the class no bound is derived for

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::spectral_sum::sup::sup_from;
use sva_formula::{NodeId, Unary, Var};
use sva_samples::biquad::{clamp_cutoff, clamp_q, design};
use sva_samples::collapse::plan::summed_bounds;
use sva_samples::{Audible, Coeffs, truncate_spectral_sum, truncate_written};

use super::floor;
use super::range::{OP, Range, Reads, TRANSFORM_OPS};
use super::ringing::Ringing;
use crate::cast::Cast;
use crate::error::EngineError;
use crate::loops::Delay;
use crate::render::RenderConfig;
use crate::typing::{Typing, Value};

/// Samples between two grid instants.
pub(crate) const STEP: usize = 256;

/// Every envelope bounds the rendered value, so each class adds the rounding its own
/// arithmetic can make: `OP` per operation, relative to the magnitudes it combines.
const OPS_PER_STEP: f64 = 64.0;

/// `at[j]` bounds `|x(s)|` for every `s` from grid instant `j` on, `before` for every `s`
/// at all, and `floor` is a level `|x|` provably returns to forever, zero where none is known.
#[derive(Clone, Debug)]
pub(crate) struct Envelope {
    pub(crate) at: Vec<f64>,
    pub(crate) before: f64,
    pub(crate) floor: f64,
}

/// The node no bound reaches, and the class it belongs to.
#[derive(Clone, Debug)]
pub(crate) struct Unbounded {
    pub(crate) node: String,
    pub(crate) class: String,
}

/// Seconds are sums of rounded terms, so an instant is read this much early: a bound taken
/// from a little before an instant covers it, one taken from a little after may not.
const SLOP: f64 = 1e-9;

/// `points` instants `STEP` samples apart, the first at grid sample `first`.
#[derive(Clone)]
pub(crate) struct Grid {
    pub(crate) first: i64,
    pub(crate) rate: f64,
    pub(crate) points: usize,
}

impl Grid {
    /// Grid instant `j`, read early by the slop.
    pub(crate) fn secs(&self, j: usize) -> f64 {
        (self.first + (j * STEP) as i64) as f64 / self.rate - SLOP
    }

    /// The last grid instant whose bound holds from `t`, where one does.
    fn index_at(&self, t: f64) -> Option<usize> {
        let steps = (((t + SLOP) * self.rate - self.first as f64) / STEP as f64).floor();
        (steps >= 0.0).then(|| (steps as usize).min(self.points - 1))
    }

    /// For each instant, the step of a solver started at t = 0 whose bound covers it.
    fn solver_steps(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.points).map(|j| {
            (self.first + (j * STEP) as i64)
                .div_euclid(STEP as i64)
                .max(0) as usize
        })
    }

    /// The same for a sample index, which a grid read lands on exactly.
    fn index_of(&self, sample: i64) -> Option<usize> {
        (sample >= 0).then(|| (sample as usize / STEP).min(self.points - 1))
    }
}

impl Envelope {
    fn constant(grid: &Grid, level: f64, floor: f64) -> Envelope {
        Envelope {
            at: vec![level; grid.points],
            before: level,
            floor,
        }
    }

    /// Each instant's bound is also every later one's.
    fn settled(mut self) -> Envelope {
        let mut held = f64::INFINITY;
        for v in &mut self.at {
            held = held.min(*v);
            *v = held;
        }
        self.before = self.before.max(self.at.first().copied().unwrap_or(0.0));
        self
    }

    fn map(mut self, f: impl Fn(f64) -> f64) -> Envelope {
        self.at.iter_mut().for_each(|v| *v = f(*v));
        self.before = f(self.before);
        self.floor = 0.0;
        self
    }

    fn zip(mut self, other: &Envelope, f: impl Fn(f64, f64) -> f64) -> Envelope {
        for (v, w) in self.at.iter_mut().zip(&other.at) {
            *v = f(*v, *w);
        }
        self.before = f(self.before, other.before);
        self.floor = 0.0;
        self
    }

    /// A buffer holds nothing before its window opens, so its bound from there is its bound
    /// over all time.
    fn held(mut self) -> Envelope {
        self.before = self.at.first().copied().unwrap_or(0.0);
        self
    }

    pub(crate) fn peak(&self) -> f64 {
        self.before
    }

    /// Every level from the last grid instant on, which bounds what `|x|` returns to forever.
    fn tail(&self) -> f64 {
        self.at.last().copied().unwrap_or(self.before)
    }
}

/// Every node's bound over one grid, each found once. No sample of a node is rendered for it.
pub(crate) struct Bounds<'a> {
    pub(crate) tys: &'a Typing,
    pub(crate) grid: Grid,
    pub(crate) config: &'a RenderConfig,
    /// How low a solver's bound is stepped down before it is held flat, and a lower level
    /// for each solver whose gain to the output needs one.
    pub(crate) level: f64,
    pub(crate) levels: BTreeMap<NodeId, f64>,
    /// What the bounds found so far cost, counted as a render's operations are.
    pub(crate) ops: u128,
    forms: &'a Forms<'a>,
    held: BTreeMap<NodeId, Result<Envelope, Unbounded>>,
    open: BTreeSet<NodeId>,
}

pub(crate) type Found = Result<Envelope, Unbounded>;

/// A closed form in `t` as a bound reads it, which no instant changes.
pub(super) enum Form {
    /// The atoms, and each direct sum over their lines as its bound and the factor it is under.
    Atoms(Vec<SpectralAtom>, Vec<(Option<SpectralAtom>, f64)>),
    Written(Range),
    Unbounded(&'static str),
}

/// Each node's form, compiled once for every proof sharing it, under the typing and config
/// it holds: a proof reads both from here, so no form is read under another.
pub(crate) struct Forms<'a> {
    pub(crate) tys: Cow<'a, Typing>,
    pub(crate) config: Cow<'a, RenderConfig>,
    compiled: RefCell<BTreeMap<NodeId, Option<Rc<Form>>>>,
}

impl<'a> Forms<'a> {
    pub(crate) fn new(tys: Cow<'a, Typing>, config: Cow<'a, RenderConfig>) -> Forms<'a> {
        Forms {
            tys,
            config,
            compiled: RefCell::default(),
        }
    }
}

impl<'a> Bounds<'a> {
    pub(crate) fn new(forms: &'a Forms<'a>, grid: Grid, level: f64) -> Bounds<'a> {
        Bounds {
            tys: &forms.tys,
            grid,
            config: &forms.config,
            level,
            levels: BTreeMap::new(),
            ops: 0,
            forms,
            held: BTreeMap::new(),
            open: BTreeSet::new(),
        }
    }

    pub(crate) fn of(&mut self, id: NodeId) -> Result<Found, EngineError> {
        if let Some(found) = self.held.get(&id) {
            return Ok(found.clone());
        }
        if !self.open.insert(id) {
            return Ok(Err(self.unknown(id, "a loop across nodes")));
        }
        let found = self.fresh(id)?.map(Envelope::settled);
        self.ops += self.grid.points as u128;
        self.open.remove(&id);
        self.held.insert(id, found.clone());
        Ok(found)
    }

    fn unknown(&self, id: NodeId, class: &str) -> Unbounded {
        Unbounded {
            node: self.tys.name(id).to_string(),
            class: class.to_string(),
        }
    }

    fn form(&self, id: NodeId) -> Option<Rc<Form>> {
        if let Some(form) = self.forms.compiled.borrow().get(&id) {
            return form.clone();
        }
        let form = self.compiled(id).map(Rc::new);
        self.forms.compiled.borrow_mut().insert(id, form.clone());
        form
    }

    fn compiled(&self, id: NodeId) -> Option<Form> {
        let tys = self.tys;
        // A series no line reaches falls to the written form, as the collapse does.
        if tys.ty(id).is_closed_form()
            && let Ok(whole) = crate::refs::spectral_sum_of(tys, id, Var::T)
            && let Ok(sum) = truncate_spectral_sum(&whole, self.band())
        {
            let (profile, rate) = (&self.config.profile, self.config.rate);
            let Ok(summed) = summed_bounds(&whole, profile, rate) else {
                return Some(Form::Unbounded("a line sum with no rounding bound"));
            };
            let atoms: Vec<SpectralAtom> = sum
                .lanes
                .iter()
                .flat_map(|lane| {
                    let modal = lane.modal.iter().flat_map(|bank| {
                        sva_formula::modal::atoms(bank, sva_formula::Origin::UNKNOWN)
                    });
                    lane.atoms.iter().copied().chain(modal)
                })
                .collect();
            return Some(Form::Atoms(atoms, summed));
        }
        let Value::ClosedForm(form) = tys.value(id) else {
            return None;
        };
        if form.var != Var::T {
            return None;
        }
        let Ok(body) = truncate_written(&form.body, self.band()) else {
            return Some(Form::Unbounded("a series with no term count"));
        };
        Some(match Range::of(&body) {
            Ok(Range::Atoms(atoms)) => Form::Atoms(atoms, Vec::new()),
            Ok(range) => Form::Written(range),
            Err(class) => Form::Unbounded(class),
        })
    }

    fn fresh(&mut self, id: NodeId) -> Result<Found, EngineError> {
        let tys = self.tys;
        if let Some(form) = self.form(id) {
            return match &*form {
                Form::Atoms(atoms, summed) => Ok(self.atoms(id, atoms, summed)),
                Form::Written(range) => self.written(id, range),
                Form::Unbounded(class) => Ok(Err(self.unknown(id, class))),
            };
        }
        match tys.value(id).clone() {
            Value::ClosedForm(_) => Ok(Err(self.unknown(id, "a closed form in f"))),
            Value::Cast(Cast::Sample, source) => Ok(self.of(source)?.map(Envelope::held)),
            Value::Cast(..) => Ok(Err(self.unknown(id, "a transform of the whole signal"))),
            Value::Read { source, at, .. } => {
                let Ok(steps) = at.steps_at(self.config.rate) else {
                    return Ok(Err(self.unknown(id, "a read between two samples")));
                };
                let e = match self.of(source)? {
                    Ok(e) => e.held(),
                    Err(e) => return Ok(Err(e)),
                };
                Ok(Ok(self.moved(&e, steps)))
            }
            Value::Grid(count) => Ok(Ok(Envelope::constant(
                &self.grid,
                (count / self.grid.rate).abs(),
                0.0,
            ))),
            Value::SelfAt(_) => Ok(Err(self.unknown(id, "a loop read outside its loop"))),
            Value::Solver(params) => {
                let rate = self.config.rate;
                let steps: Vec<usize> = self.grid.solver_steps().collect();
                let reach = steps.last().copied().unwrap_or(0) + 1;
                self.ops += (reach * STEP) as u128;
                let level = self.levels.get(&id).copied().unwrap_or(self.level);
                match sva_samples::tail(&params, rate, STEP, reach, level) {
                    Ok(tail) => Ok(Ok(Envelope {
                        before: tail.at.first().copied().unwrap_or(0.0),
                        at: steps.iter().map(|q| tail.at[(*q).min(reach - 1)]).collect(),
                        floor: 0.0,
                    })),
                    Err(class) => Ok(Err(self.unknown(id, &class))),
                }
            }
            Value::Filter {
                shape,
                x,
                cutoff,
                q,
                gain,
            } => {
                let numbers = [cutoff, q, gain].map(|p| constant(tys, p));
                let [Some(cutoff), Some(q), Some(gain)] = numbers else {
                    return Ok(Err(self.unknown(id, "a filter whose coefficients move")));
                };
                let rate = self.grid.rate;
                let coeffs = design(
                    shape,
                    clamp_cutoff(cutoff, rate).0,
                    clamp_q(q).0,
                    gain,
                    rate,
                );
                let recursion = Coeffs {
                    b0: 1.0,
                    b1: 0.0,
                    b2: 0.0,
                    ..coeffs
                };
                let (Some(ringing), Some(recursion)) =
                    (Ringing::of(&coeffs), Ringing::of(&recursion))
                else {
                    return Ok(Err(self.unknown(id, "a filter that never settles")));
                };
                let input = match self.of(x)? {
                    Ok(e) => e,
                    Err(e) => return Ok(Err(e)),
                };
                Ok(Ok(self.filtered(&input, &ringing, &recursion, &coeffs)))
            }
            Value::Op { name, args } => match holds_self(tys, id) {
                true => self.looped(id),
                false => self.operation(id, &name, &args),
            },
        }
    }

    fn band(&self) -> Audible {
        Audible::of(&self.config.profile, self.config.rate)
    }

    /// Read `steps` later: a read landing before the grid's start takes the source's bound
    /// over all time.
    fn moved(&self, e: &Envelope, steps: i64) -> Envelope {
        let at: Vec<f64> = (0..self.grid.points)
            .map(|j| match self.grid.index_of((j * STEP) as i64 + steps) {
                Some(i) => e.at[i],
                None => e.before,
            })
            .collect();
        Envelope {
            before: e.before.max(at.first().copied().unwrap_or(0.0)),
            at,
            floor: e.floor,
        }
    }

    /// Each atom's own supremum from every instant, summed, and each direct sum's rounding
    /// under its factor's supremum.
    fn atoms(
        &mut self,
        id: NodeId,
        atoms: &[SpectralAtom],
        summed: &[(Option<SpectralAtom>, f64)],
    ) -> Found {
        let mut at = vec![0.0; self.grid.points];
        let mut before = 0.0;
        let (points, count) = (self.grid.points as u128, atoms.len() as u128);
        self.ops += points * count;
        for atom in atoms {
            for (j, v) in at.iter_mut().enumerate() {
                let Some(sup) = sup_from(atom, self.grid.secs(j)) else {
                    return Err(self.unknown(id, "a delta or a pole on the line"));
                };
                *v += sup;
            }
            before += sup_from(atom, f64::NEG_INFINITY).unwrap_or(f64::INFINITY);
        }
        let direct = |t: f64| {
            summed.iter().try_fold(0.0, |held, (factor, err)| {
                let under = factor.as_ref().map_or(Some(1.0), |f| sup_from(f, t))?;
                Some(held + err * under)
            })
        };
        let rounded = 1.0 + OP * (atoms.len() as f64 + TRANSFORM_OPS);
        let mut bounded = Vec::with_capacity(at.len());
        for (j, v) in at.into_iter().enumerate() {
            let Some(err) = direct(self.grid.secs(j)) else {
                return Err(self.unknown(id, "a delta or a pole on the line"));
            };
            bounded.push(v * rounded + err);
        }
        let everywhere = direct(f64::NEG_INFINITY).unwrap_or(f64::INFINITY);
        let floor = floor::of_atoms(atoms, self.grid.rate) * (2.0 - rounded) - everywhere;
        Ok(Envelope {
            at: bounded,
            before: before * rounded + everywhere,
            floor: floor.max(0.0),
        })
    }

    /// A written closed form no atom sum reaches, bounded by interval arithmetic over every
    /// instant from each grid point on.
    fn written(&mut self, id: NodeId, range: &Range) -> Result<Found, EngineError> {
        self.ops += (self.grid.points * range.size()) as u128;
        let mut nodes = Vec::new();
        range.nodes(&mut nodes);
        let mut held = BTreeMap::new();
        for node in nodes {
            match self.of(node)? {
                Ok(e) => held.insert(node, e),
                Err(e) => return Ok(Err(e)),
            };
        }
        let grid = &self.grid;
        let read = |node: NodeId, t: f64| {
            let e = &held[&node];
            match grid.index_at(t) {
                Some(i) => e.at[i],
                None => e.before,
            }
        };
        let magnitude = |t: f64| range.from(t, &read).map(|s| s.reach() + s.err);
        let floor_of = |node: NodeId| held[&node].floor;
        let reads = Reads {
            node: &read,
            floor: &floor_of,
            rate: grid.rate,
        };
        let last = grid.secs(grid.points - 1);
        let rounding = range.from(last, &read).map_or(f64::INFINITY, |s| s.err);
        let floor = (range.floor(last, &reads) - rounding).max(0.0);
        let mut at = Vec::with_capacity(grid.points);
        for j in 0..grid.points {
            match magnitude(grid.secs(j)) {
                Some(v) => at.push(v),
                None => return Ok(Err(self.unknown(id, "a division by what may be zero"))),
            }
        }
        Ok(Ok(Envelope {
            before: magnitude(f64::NEG_INFINITY).unwrap_or(f64::INFINITY),
            at,
            floor,
        }))
    }

    fn cropped(&self, e: &Envelope, l: f64, r: f64) -> Envelope {
        let from = |t: f64| match self.grid.index_at(t.max(l)) {
            Some(i) => e.at[i],
            None => e.before,
        };
        Envelope {
            at: (0..self.grid.points)
                .map(|j| match self.grid.secs(j) >= r {
                    true => 0.0,
                    false => from(self.grid.secs(j)),
                })
                .collect(),
            before: from(f64::NEG_INFINITY),
            floor: match r.is_finite() {
                true => 0.0,
                false => e.floor,
            },
        }
    }

    /// Every sampled operation, over its operands' own bounds.
    fn operation(&mut self, id: NodeId, name: &str, args: &[NodeId]) -> Result<Found, EngineError> {
        if name == "crop" {
            return self.crop(id, args);
        }
        let mut held = Vec::with_capacity(args.len());
        for arg in args {
            match self.of(*arg)? {
                Ok(e) => held.push(e),
                Err(e) => return Ok(Err(e)),
            }
        }
        let numbers: Vec<Option<f64>> = args.iter().map(|a| constant(self.tys, *a)).collect();
        let rounded = 1.0 + OP * args.len() as f64;
        let joined = |held: Vec<Envelope>, f: fn(f64, f64) -> f64| {
            let mut it = held.into_iter();
            let first = it.next().expect("an operation with operands");
            it.fold(first, |acc, e| acc.zip(&e, f))
        };
        let floors: Vec<f64> = held.iter().map(|e| e.floor).collect();
        let tails: Vec<f64> = held.iter().map(Envelope::tail).collect();
        let constants: f64 = numbers.iter().flatten().map(|k| k.abs()).product();
        let moving: Vec<f64> = numbers
            .iter()
            .zip(&floors)
            .filter(|(k, _)| k.is_none())
            .map(|(_, f)| *f)
            .collect();
        let mut exact = match name {
            "+" | "-" => joined(held, |a, b| a + b),
            "*" => joined(held, |a, b| a * b),
            "/" => match numbers.get(1).copied().flatten() {
                Some(d) if d != 0.0 => held[0].clone().map(|v| v / d.abs()),
                _ => return Ok(Err(self.unknown(id, "a division by a moving signal"))),
            },
            "max" | "min" | "join" => joined(held, f64::max),
            "ch" => held[0].clone(),
            "pow" => match numbers.get(1).copied().flatten() {
                Some(n) if n >= 1.0 && n.fract() == 0.0 => {
                    held[0].clone().map(|v| v.powi(n as i32))
                }
                _ => return Ok(Err(self.unknown(id, "a power that is not a whole one"))),
            },
            other => match Unary::from_name(other).and_then(mapped) {
                Some(f) => held[0].clone().map(f),
                None => return Ok(Err(self.unknown(id, &format!("`{other}` of a signal")))),
            },
        };
        exact.floor = match (name, moving.as_slice()) {
            ("+" | "-", _) => floor::summed(&floors, &tails),
            ("*", [one]) => one * constants,
            ("/", _) => floors[0] / numbers[1].map_or(f64::INFINITY, f64::abs),
            ("pow", _) => floors[0].powi(numbers[1].map_or(0, |n| n as i32)),
            ("join", _) => floors.iter().copied().fold(0.0, f64::max),
            ("max" | "min", _) => floor::bounded_away(name, &numbers),
            (other, _) => Unary::from_name(other).map_or(0.0, |op| floor::through(op, floors[0])),
        };
        let floor = exact.floor * (2.0 - rounded);
        let mut out = exact.map(|v| v * rounded);
        out.floor = floor;
        Ok(Ok(out))
    }

    /// Past a crop's end every sample is zero, whether or not a bound reaches its operand.
    fn crop(&mut self, id: NodeId, args: &[NodeId]) -> Result<Found, EngineError> {
        let edge = |at: usize| args.get(at).and_then(|a| constant(self.tys, *a));
        let (Some(l), Some(r)) = (edge(1), edge(2)) else {
            return Ok(Err(self.unknown(id, "a crop whose window moves")));
        };
        match self.of(args[0])? {
            Ok(e) => Ok(Ok(self.cropped(&e, l, r))),
            Err(_) if r.is_finite() => {
                let unknown = Envelope::constant(&self.grid, f64::INFINITY, 0.0);
                Ok(Ok(self.cropped(&unknown, l, r)))
            }
            Err(e) => Ok(Err(e)),
        }
    }

    /// `|y| <= |h| * |x|`: the input from each grid instant on meets the whole response, and
    /// each earlier block of it only the response past the lags between.
    /// Each step rounds by `OP` of what it sums, and that error runs through the recursion
    /// alone, whose own response `recursion` bounds.
    fn filtered(
        &self,
        x: &Envelope,
        ringing: &Ringing,
        recursion: &Ringing,
        c: &Coeffs,
    ) -> Envelope {
        let points = self.grid.points;
        let lag: Vec<f64> = (1..=points).map(|g| ringing.window(g - 1)).collect();
        let mut after = lag.clone();
        for g in (0..points.saturating_sub(1)).rev() {
            after[g] += after[g + 1];
        }
        // Blocks further back reach the output by 1e-18 of the response, carried in `rest`.
        let negligible = (ringing.sum * 1e-18).max(f64::MIN_POSITIVE);
        let reach = after
            .iter()
            .position(|rest| *rest <= negligible)
            .unwrap_or(points);
        let largest = x.before;
        let rest = after.get(reach).copied().unwrap_or(0.0) * largest;
        let feed = c.b0.abs() + c.b1.abs() + c.b2.abs();
        let back = c.a1.abs() + c.a2.abs();
        let slack = recursion.sum * OP * (feed + back * ringing.sum) * largest * 2.0;
        let at = (0..points)
            .map(|j| {
                let near: f64 = (1..=j.min(reach)).map(|g| lag[g - 1] * x.at[j - g]).sum();
                ringing.sum * x.at[j] + near + rest + slack
            })
            .collect();
        Envelope {
            at,
            before: ringing.sum * largest + slack,
            floor: 0.0,
        }
    }

    /// `|y(n)| <= |F(n)| + sum g_i |y(n - D_i)|` with `sum g_i < 1`, iterated on the grid.
    /// One tap is unrolled across a whole step, so a short delay decays per delay rather
    /// than per step.
    fn looped(&mut self, id: NodeId) -> Result<Found, EngineError> {
        let mut form = match self.affine(id, id)? {
            Ok(form) => form,
            Err(e) => return Ok(Err(e)),
        };
        let gain: f64 = form.taps.iter().map(|(g, _)| g).sum();
        if gain >= 1.0 {
            return Ok(Err(self.unknown(id, "a loop whose gain does not contract")));
        }
        let free = form
            .free
            .take()
            .unwrap_or_else(|| Envelope::constant(&self.grid, 0.0, 0.0));
        let points = self.grid.points;
        let largest = free.before / (1.0 - gain);
        let slack = OP * OPS_PER_STEP * (1.0 + gain) * largest / (1.0 - gain);
        let mut at = vec![0.0f64; points];
        at[0] = largest;
        let read = |at: &[f64], sample: i64| match self.grid.index_of(sample) {
            Some(i) => at[i],
            None => at[0],
        };
        for j in 1..points {
            let n = (j * STEP) as i64;
            at[j] = match form.taps.as_slice() {
                [(g, d)] => {
                    let times = (STEP / *d).max(1);
                    let mut held = 0.0;
                    let mut weight = 1.0;
                    for k in 0..times {
                        held += weight * read(&free.at, n - (k * d) as i64).min(free.before);
                        weight *= g;
                    }
                    held + weight * read(&at, n - (times * d) as i64)
                }
                taps => {
                    free.at[j]
                        + taps
                            .iter()
                            .map(|(g, d)| g * read(&at, n - *d as i64))
                            .sum::<f64>()
                }
            };
            at[j] = at[j].min(at[j - 1]);
        }
        Ok(Ok(Envelope {
            at: at.into_iter().map(|v| v + slack).collect(),
            before: largest + slack,
            floor: 0.0,
        }))
    }

    /// The loop body as a bound linear in its own delayed output. A map with `|f(u)| <= |u|`
    /// keeps a bound of that shape, and so does a product with a signal bounded everywhere.
    fn affine(
        &mut self,
        owner: NodeId,
        id: NodeId,
    ) -> Result<Result<Affine, Unbounded>, EngineError> {
        if !holds_self(self.tys, id) {
            return Ok(self.of(id)?.map(|e| Affine {
                free: Some(e),
                taps: Vec::new(),
            }));
        }
        let value = self.tys.value(id).clone();
        let refuse = |b: &Bounds, what: &str| Ok(Err(b.unknown(owner, what)));
        match value {
            Value::SelfAt(delay) => match self.steps(delay) {
                Some(d) => Ok(Ok(Affine {
                    free: None,
                    taps: vec![(1.0, d)],
                })),
                None => refuse(self, "a loop whose delay moves"),
            },
            Value::Op { name, args } => {
                let mut forms = Vec::with_capacity(args.len());
                for arg in &args {
                    match self.affine(owner, *arg)? {
                        Ok(f) => forms.push(f),
                        Err(e) => return Ok(Err(e)),
                    }
                }
                match name.as_str() {
                    "+" | "-" => Ok(Ok(forms.into_iter().fold(Affine::default(), Affine::add))),
                    "*" | "/" => {
                        let looped = forms.iter().filter(|f| !f.taps.is_empty()).count();
                        if looped != 1 {
                            return refuse(self, "a loop multiplied by itself");
                        }
                        let mut out = Affine::default();
                        let mut scale = 1.0;
                        for (at, form) in forms.into_iter().enumerate() {
                            match (form.taps.is_empty(), name.as_str(), at) {
                                (false, _, _) => out = form,
                                (true, "*", _) => scale *= form.peak(),
                                (true, _, 1) => match constant(self.tys, args[1]) {
                                    Some(d) if d != 0.0 => scale /= d.abs(),
                                    _ => return refuse(self, "a loop divided by a signal"),
                                },
                                (true, _, _) => return refuse(self, "a loop under a division"),
                            }
                        }
                        Ok(Ok(out.scaled(scale)))
                    }
                    "crop" | "tanh" | "sat" | "sin" | "abs" => {
                        Ok(Ok(forms.into_iter().next().expect("an operand")))
                    }
                    _ => refuse(self, "a loop through a map with no contraction"),
                }
            }
            _ => refuse(self, "a loop through a filter or a transform"),
        }
    }

    fn steps(&self, delay: Delay) -> Option<usize> {
        match delay {
            Delay::Steps(steps) => Some(steps as usize),
            Delay::Secs(secs) => Some(((secs * self.grid.rate).round() as usize).max(1)),
            Delay::Varying => None,
        }
    }
}

/// `|body| <= |free| + sum g |y(n - d)|`.
#[derive(Default)]
struct Affine {
    free: Option<Envelope>,
    taps: Vec<(f64, usize)>,
}

impl Affine {
    fn add(mut self, other: Affine) -> Affine {
        self.free = match (self.free, other.free) {
            (Some(a), Some(b)) => Some(a.zip(&b, |x, y| x + y)),
            (a, b) => a.or(b),
        };
        self.taps.extend(other.taps);
        self
    }

    fn scaled(mut self, k: f64) -> Affine {
        self.free = self.free.map(|e| e.map(|v| v * k));
        self.taps.iter_mut().for_each(|(g, _)| *g *= k);
        self
    }

    fn peak(&self) -> f64 {
        self.free.as_ref().map_or(0.0, Envelope::peak)
    }
}

/// `|f(u)| <= g(|u|)` for a map that keeps zero at zero; `None` for one that moves it or grows.
fn mapped(op: Unary) -> Option<fn(f64) -> f64> {
    match op {
        Unary::Tanh | Unary::Sat | Unary::Sin => Some(|v: f64| v.min(1.0)),
        Unary::Abs => Some(|v| v),
        Unary::Sqrt => Some(f64::sqrt),
        Unary::Exp | Unary::Cos | Unary::Log => None,
    }
}

fn constant(tys: &Typing, id: NodeId) -> Option<f64> {
    match tys.value(id) {
        Value::ClosedForm(form) if crate::lower::never(&form.body) => Some(f64::INFINITY),
        Value::ClosedForm(form) => crate::lower::constant_value(&form.body, form.var),
        _ => None,
    }
}

fn holds_self(tys: &Typing, id: NodeId) -> bool {
    crate::schedule::holds_self(tys, id, &mut BTreeSet::new())
}
