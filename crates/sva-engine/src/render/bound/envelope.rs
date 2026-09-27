// Concern: bounds every node's magnitude from each grid instant on, instant by instant | Non-concern: where a node is cut by the bound (cut/) | IO: (NodeId, instants) -> bounds, or why none

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::spectral_sum::sup::sup_from;
use sva_formula::{NodeId, Unary, Var};
use sva_samples::biquad::{clamp_cutoff, clamp_q, design};
use sva_samples::collapse::plan::summed_bounds;
use sva_samples::{Audible, truncate_spectral_sum, truncate_written};

use super::filtered::Filtered;
use super::floor;
use super::looped::Looped;
use super::range::{OP, Range, Reads, TRANSFORM_OPS};
use super::solver::{self, Fresh, Played, Solver};
use crate::cast::Cast;
use crate::error::EngineError;
use crate::render::RenderConfig;
use crate::typing::{Typing, Value};

/// Samples between two grid instants.
pub(crate) const STEP: usize = 256;

/// Seconds are sums of rounded terms, so an instant is read this much early: a bound taken
/// from a little before an instant covers it, one taken from a little after may not.
const SLOP: f64 = 1e-9;

/// How many instants past its own a bound reads ahead; a read further on takes the bound from
/// that instant, which holds from there on, so no instant waits on a distant one.
const AHEAD: usize = 1 << 12;

/// Instants `STEP` samples apart from grid sample `first` on, as many as any bound reads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Grid {
    pub(crate) first: i64,
    pub(crate) rate: f64,
}

impl Grid {
    /// Grid instant `j`, read early by the slop.
    pub(crate) fn secs(&self, j: usize) -> f64 {
        self.sample(j) as f64 / self.rate - SLOP
    }

    pub(crate) fn sample(&self, j: usize) -> i64 {
        self.first + (j * STEP) as i64
    }

    /// The last grid instant whose bound holds from `t`, where one does.
    fn index_at(&self, t: f64) -> Option<usize> {
        let steps = (((t + SLOP) * self.rate - self.first as f64) / STEP as f64).floor();
        (steps >= 0.0).then_some(steps as usize)
    }

    /// The chunk of a solver started at t = 0 whose bound covers instant `j`.
    pub(crate) fn chunk(&self, j: usize) -> usize {
        self.sample(j).div_euclid(STEP as i64).max(0) as usize
    }
}

/// The node no bound reaches, and the class it belongs to.
#[derive(Clone, Debug)]
pub(crate) struct Unbounded {
    pub(crate) node: String,
    pub(crate) class: String,
}

/// Each node's bound found so far: `at[j]` bounds `|x(s)|` for every `s` from grid instant `j`
/// on, `before` for every `s` at all once `at[0]` is.
struct Slot {
    class: Class,
    at: Vec<f64>,
    before: f64,
}

enum Class {
    Constant(f64),
    Atoms(Rc<Form>, f64),
    Written(Rc<Form>),
    Held(NodeId),
    Moved(NodeId, i64),
    Solver(Box<Solver>),
    Filter(Box<Filtered>),
    Loop(Box<Looped>),
    /// An operation, its operands, and each operand's constant value where it has one.
    Op(String, Vec<NodeId>, Vec<Option<f64>>),
    /// A crop's operand, where a bound reaches it, and its window.
    Crop(Option<NodeId>, f64, f64),
}

/// Where extending a node stops: every instant asked for is bounded, or one waits on a
/// solver's chunk no walk has heard yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reach {
    Ready,
    Wait(NodeId, usize),
}

/// One instant of one node, or what it needs first.
enum Point {
    Value(f64),
    Wait(NodeId, usize),
    Needs(NodeId, usize),
    Unbounded(Unbounded),
}

/// Every node's bound, grown one instant at a time; an instant once bounded never changes, so
/// a bound read further is the bound read so far and more. No sample of a node is rendered.
pub(crate) struct Bounds<'a> {
    forms: Rc<Forms<'a>>,
    pub(crate) grid: Grid,
    /// How low a solver's bound is stepped before it is held flat.
    pub(crate) level: f64,
    /// The most chunks a solver's bound steps before its bound is taken as unbounded.
    limit: usize,
    /// What the bounds found so far cost, counted as a render's operations are.
    pub(crate) ops: u128,
    slots: BTreeMap<NodeId, Result<Slot, Unbounded>>,
    open: BTreeSet<NodeId>,
}

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
    pub(crate) fn new(forms: Rc<Forms<'a>>, grid: Grid, level: f64) -> Bounds<'a> {
        let budget = forms.config.flop_budget / STEP as u128;
        Bounds {
            forms,
            grid,
            level,
            limit: usize::try_from(budget).unwrap_or(usize::MAX),
            ops: 0,
            slots: BTreeMap::new(),
            open: BTreeSet::new(),
        }
    }

    pub(crate) fn tys(&self) -> &Typing {
        &self.forms.tys
    }

    pub(crate) fn config(&self) -> &RenderConfig {
        &self.forms.config
    }

    /// Why no bound reaches `id`, once it is built.
    pub(crate) fn unbounded(&self, id: NodeId) -> Option<&Unbounded> {
        self.slots.get(&id)?.as_ref().err()
    }

    fn slot(&self, id: NodeId) -> &Slot {
        match self.slots.get(&id) {
            Some(Ok(slot)) => slot,
            _ => panic!("node {id:?} is read before it is bounded"),
        }
    }

    /// Instants bounded so far.
    pub(crate) fn len(&self, id: NodeId) -> usize {
        match self.slots.get(&id) {
            Some(Ok(slot)) => slot.at.len(),
            _ => 0,
        }
    }

    pub(crate) fn at(&self, id: NodeId, j: usize) -> f64 {
        self.slot(id).at[j]
    }

    /// Its bound over all time, once its first instant is bounded; `None` where no bound is.
    pub(crate) fn peak(&self, id: NodeId) -> Option<f64> {
        match self.slots.get(&id)? {
            Ok(slot) if !slot.at.is_empty() => Some(slot.before),
            _ => None,
        }
    }

    /// The bound from the last instant found on.
    pub(crate) fn tail(&self, id: NodeId) -> f64 {
        let slot = self.slot(id);
        slot.at.last().copied().unwrap_or(slot.before)
    }

    pub(crate) fn solver_mut(&mut self, id: NodeId) -> Option<&mut Solver> {
        match self.slots.get_mut(&id)? {
            Ok(Slot {
                class: Class::Solver(tail),
                ..
            }) => Some(tail),
            _ => None,
        }
    }

    pub(crate) fn solver(&self, id: NodeId) -> Option<&Solver> {
        match self.slots.get(&id)? {
            Ok(Slot {
                class: Class::Solver(tail),
                ..
            }) => Some(tail),
            _ => None,
        }
    }

    /// Plays `solver`'s own machine on until its walk knows `chunk`, from `fresh` where none
    /// is played ahead.
    pub(crate) fn play(&mut self, solver: NodeId, chunk: usize, fresh: Fresh) {
        let limit = self.limit;
        if let Some(tail) = self.solver_mut(solver) {
            solver::play(tail, chunk, fresh, limit);
        }
    }

    /// What `solver`'s own machine played for its bound, taken to write its samples.
    pub(crate) fn played(&mut self, solver: NodeId) -> Option<Played> {
        self.solver_mut(solver)?.ahead.take()
    }

    /// Whether a solver's walk still reads its site's states.
    pub(crate) fn walking(&self, id: NodeId) -> bool {
        self.solver(id).is_some_and(|t| !t.lost && !t.walk.done())
    }

    /// Every solver whose bound is built, each stepped by its own walk.
    pub(crate) fn solvers(&self) -> Vec<NodeId> {
        self.slots
            .iter()
            .filter(|(_, slot)| {
                matches!(
                    slot,
                    Ok(Slot {
                        class: Class::Solver(_),
                        ..
                    })
                )
            })
            .map(|(id, _)| *id)
            .collect()
    }

    fn unknown(&self, id: NodeId, class: &str) -> Unbounded {
        Unbounded {
            node: self.tys().name(id).to_string(),
            class: class.to_string(),
        }
    }

    /// Builds `id`'s bound, no instant of it yet; `Err` where none is derived.
    pub(crate) fn of(&mut self, id: NodeId) -> Result<Result<(), Unbounded>, EngineError> {
        if let Some(held) = self.slots.get(&id) {
            return Ok(held.as_ref().map(|_| ()).map_err(Clone::clone));
        }
        if !self.open.insert(id) {
            return Ok(Err(self.unknown(id, "a loop across nodes")));
        }
        let class = self.class(id);
        self.open.remove(&id);
        let class = class?;
        let found = class.as_ref().map(|_| ()).map_err(Clone::clone);
        let slot = class.map(|class| Slot {
            class,
            at: Vec::new(),
            before: 0.0,
        });
        self.slots.insert(id, slot);
        Ok(found)
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
        let tys = self.tys();
        // A series no line reaches falls to the written form, as the collapse does.
        if tys.ty(id).is_closed_form()
            && let Ok(whole) = crate::refs::spectral_sum_of(tys, id, Var::T)
            && let Ok(sum) = truncate_spectral_sum(&whole, self.band())
        {
            let (profile, rate) = (&self.config().profile, self.config().rate);
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

    fn band(&self) -> Audible {
        Audible::of(&self.config().profile, self.config().rate)
    }

    /// An operand's own bound, or why none is.
    fn operand(&mut self, id: NodeId) -> Result<Result<NodeId, Unbounded>, EngineError> {
        Ok(self.of(id)?.map(|()| id))
    }

    fn class(&mut self, id: NodeId) -> Result<Result<Class, Unbounded>, EngineError> {
        let forms = self.forms.clone();
        let tys = &*forms.tys;
        if let Some(form) = self.form(id) {
            return Ok(match &*form {
                Form::Atoms(atoms, _) => {
                    let rounded = 1.0 + OP * (atoms.len() as f64 + TRANSFORM_OPS);
                    let everywhere = direct(&form, f64::NEG_INFINITY).unwrap_or(f64::INFINITY);
                    let floor = floor::of_atoms(atoms, self.grid.rate) * (2.0 - rounded);
                    Ok(Class::Atoms(form.clone(), (floor - everywhere).max(0.0)))
                }
                Form::Written(range) => {
                    let mut nodes = Vec::new();
                    range.nodes(&mut nodes);
                    for node in nodes {
                        if let Err(e) = self.of(node)? {
                            return Ok(Err(e));
                        }
                    }
                    Ok(Class::Written(form.clone()))
                }
                Form::Unbounded(class) => Err(self.unknown(id, class)),
            });
        }
        Ok(match tys.value(id).clone() {
            Value::ClosedForm(_) => Err(self.unknown(id, "a closed form in f")),
            Value::Cast(Cast::Sample, source) => self.operand(source)?.map(Class::Held),
            Value::Cast(..) => Err(self.unknown(id, "a transform of the whole signal")),
            Value::Read { source, at, .. } => match at.steps_at(self.config().rate) {
                Ok(steps) => self.operand(source)?.map(|s| Class::Moved(s, steps)),
                Err(_) => Err(self.unknown(id, "a read between two samples")),
            },
            Value::Grid(count) => Ok(Class::Constant((count / self.grid.rate).abs())),
            Value::SelfAt(_) => Err(self.unknown(id, "a loop read outside its loop")),
            Value::Solver(params) => match Solver::of(&params) {
                Ok(tail) => Ok(Class::Solver(Box::new(tail))),
                Err(class) => Err(self.unknown(id, &class)),
            },
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
                match Filtered::of(x, &coeffs) {
                    None => Err(self.unknown(id, "a filter that never settles")),
                    Some(filtered) => self.operand(x)?.map(|_| Class::Filter(Box::new(filtered))),
                }
            }
            Value::Op { .. } if holds_self(tys, id) => {
                Ok(Class::Loop(Box::new(Looped::pending(id))))
            }
            Value::Op { name, args } => self.operation(id, name, args)?,
        })
    }

    fn operation(
        &mut self,
        id: NodeId,
        name: String,
        args: Vec<NodeId>,
    ) -> Result<Result<Class, Unbounded>, EngineError> {
        let forms = self.forms.clone();
        let numbers: Vec<Option<f64>> = args.iter().map(|a| constant(&forms.tys, *a)).collect();
        if name == "crop" {
            let (Some(Some(l)), Some(Some(r))) = (numbers.get(1), numbers.get(2)) else {
                return Ok(Err(self.unknown(id, "a crop whose window moves")));
            };
            if l >= r {
                return Ok(Ok(Class::Constant(0.0)));
            }
            return Ok(match self.of(args[0])? {
                Ok(()) => Ok(Class::Crop(Some(args[0]), *l, *r)),
                Err(_) if r.is_finite() => Ok(Class::Crop(None, *l, *r)),
                Err(e) => Err(e),
            });
        }
        for arg in &args {
            if let Err(e) = self.of(*arg)? {
                return Ok(Err(e));
            }
        }
        let refused = match name.as_str() {
            "+" | "-" | "*" | "max" | "min" | "join" | "ch" => None,
            "/" => match numbers.get(1).copied().flatten() {
                Some(d) if d != 0.0 => None,
                _ => Some("a division by a moving signal".to_string()),
            },
            "pow" => match numbers.get(1).copied().flatten() {
                Some(n) if n >= 1.0 && n.fract() == 0.0 => None,
                _ => Some("a power that is not a whole one".to_string()),
            },
            other => match Unary::from_name(other).and_then(mapped) {
                Some(_) => None,
                None => Some(format!("`{other}` of a signal")),
            },
        };
        Ok(match refused {
            Some(class) => Err(self.unknown(id, &class)),
            None => Ok(Class::Op(name, args, numbers)),
        })
    }

    /// Bounds `id` over its first `len` instants, each read only once every one it reads is.
    pub(crate) fn extend(&mut self, id: NodeId, len: usize) -> Result<Reach, EngineError> {
        if self.of(id)?.is_err() {
            return Ok(Reach::Ready);
        }
        loop {
            let j = self.len(id);
            if j >= len {
                return Ok(Reach::Ready);
            }
            let inputs = self.inputs(id, j);
            for &(input, need) in &inputs {
                if let Reach::Wait(node, chunk) = self.extend(input, need)? {
                    return Ok(Reach::Wait(node, chunk));
                }
            }
            if self.unbounded_input(id, &inputs) {
                match self.unbounded(id) {
                    Some(_) => return Ok(Reach::Ready),
                    None => continue,
                }
            }
            let Some(Ok(mut slot)) = self.slots.remove(&id) else {
                return Ok(Reach::Ready);
            };
            let point = self.point(id, &mut slot, j);
            let point = match point {
                Ok(point) => point,
                Err(e) => {
                    self.slots.insert(id, Ok(slot));
                    return Err(e);
                }
            };
            match point {
                Point::Value(v) => {
                    let held = slot.at.last().copied().unwrap_or(f64::INFINITY);
                    slot.at.push(held.min(v));
                    if j == 0 {
                        slot.before = slot.before.max(slot.at[0]);
                    }
                    self.ops += 1;
                    self.slots.insert(id, Ok(slot));
                }
                Point::Wait(node, chunk) => {
                    self.slots.insert(id, Ok(slot));
                    return Ok(Reach::Wait(node, chunk));
                }
                Point::Needs(node, need) => {
                    self.slots.insert(id, Ok(slot));
                    if let Reach::Wait(node, chunk) = self.extend(node, need)? {
                        return Ok(Reach::Wait(node, chunk));
                    }
                }
                Point::Unbounded(unknown) => {
                    self.slots.insert(id, Err(unknown));
                    return Ok(Reach::Ready);
                }
            }
        }
    }

    /// An operand no bound reaches, found once its own first instant is: a crop that ends
    /// holds unbounded values inside its window, and every other class is unbounded itself.
    fn unbounded_input(&mut self, id: NodeId, inputs: &[(NodeId, usize)]) -> bool {
        let Some(unknown) = inputs.iter().find_map(|(i, _)| self.unbounded(*i).cloned()) else {
            return false;
        };
        let Some(Ok(slot)) = self.slots.get_mut(&id) else {
            return false;
        };
        match slot.class {
            Class::Loop(_) => return false,
            Class::Crop(Some(_), l, r) if r.is_finite() => slot.class = Class::Crop(None, l, r),
            _ => {
                self.slots.insert(id, Err(unknown));
            }
        }
        true
    }

    /// The nodes instant `j` of `id` reads, and how many instants of each.
    fn inputs(&self, id: NodeId, j: usize) -> Vec<(NodeId, usize)> {
        let slot = self.slot(id);
        match &slot.class {
            Class::Constant(_) | Class::Atoms(..) | Class::Solver(_) | Class::Crop(None, ..) => {
                Vec::new()
            }
            Class::Written(form) => {
                let Form::Written(range) = &**form else {
                    unreachable!("a written class holds a written form")
                };
                let mut nodes = Vec::new();
                range.nodes(&mut nodes);
                nodes.into_iter().map(|n| (n, j + 1)).collect()
            }
            Class::Held(source) => vec![(*source, j + 1)],
            Class::Filter(filtered) => vec![(filtered.x, j + 1)],
            Class::Moved(source, steps) => {
                let need = self.moved(j, *steps).map_or(1, |i| i + 1);
                vec![(*source, need.max(1))]
            }
            Class::Loop(looped) => looped.inputs(self.tys(), j),
            Class::Op(_, args, _) => args.iter().map(|a| (*a, j + 1)).collect(),
            Class::Crop(Some(x), l, r) => {
                let t = self.grid.secs(j);
                let mut from = Vec::new();
                if j == 0 {
                    from.push(*l);
                }
                if t < *r {
                    from.push(t.max(*l));
                }
                let need = from
                    .iter()
                    .filter_map(|u| self.ahead(j, self.grid.index_at(*u)))
                    .map(|i| i + 1)
                    .max();
                match from.is_empty() {
                    true => Vec::new(),
                    false => vec![(*x, need.unwrap_or(1).max(1))],
                }
            }
        }
    }

    /// Instant `i`, or the furthest a bound reads ahead of instant `j`.
    fn ahead(&self, j: usize, i: Option<usize>) -> Option<usize> {
        i.map(|i| i.min(j + AHEAD))
    }

    /// The instant a read `steps` later lands in, where it lands on the grid.
    fn moved(&self, j: usize, steps: i64) -> Option<usize> {
        let sample = (j * STEP) as i64 + steps;
        let i = (sample >= 0).then_some(sample as usize / STEP);
        self.ahead(j, i)
    }

    /// `id`'s bound from instant `i`, its bound over all time before the grid.
    fn read(&self, id: NodeId, i: Option<usize>) -> f64 {
        let slot = self.slot(id);
        match i {
            Some(i) => slot.at[i],
            None => slot.before,
        }
    }

    fn point(&mut self, id: NodeId, slot: &mut Slot, j: usize) -> Result<Point, EngineError> {
        let t = self.grid.secs(j);
        Ok(match &mut slot.class {
            Class::Constant(v) => {
                slot.before = *v;
                Point::Value(*v)
            }
            Class::Atoms(form, _) => {
                let Form::Atoms(atoms, _) = &**form else {
                    unreachable!("an atom class holds atoms")
                };
                self.ops += atoms.len() as u128;
                let rounded = 1.0 + OP * (atoms.len() as f64 + TRANSFORM_OPS);
                if j == 0 {
                    let mut before = 0.0;
                    for atom in atoms {
                        before += sup_from(atom, f64::NEG_INFINITY).unwrap_or(f64::INFINITY);
                    }
                    let everywhere = direct(form, f64::NEG_INFINITY).unwrap_or(f64::INFINITY);
                    slot.before = before * rounded + everywhere;
                }
                let mut sum = 0.0;
                let mut bounded = true;
                for atom in atoms {
                    match sup_from(atom, t) {
                        Some(sup) => sum += sup,
                        None => bounded = false,
                    }
                }
                match (bounded.then_some(()).and_then(|()| direct(form, t)), j) {
                    (Some(err), _) => Point::Value(sum * rounded + err),
                    (None, 0) => {
                        Point::Unbounded(self.unknown(id, "a delta or a pole on the line"))
                    }
                    (None, _) => Point::Value(f64::INFINITY),
                }
            }
            Class::Written(form) => {
                let Form::Written(range) = &**form else {
                    unreachable!("a written class holds a written form")
                };
                self.ops += range.size() as u128;
                let missing = Cell::new(None);
                let read = |node: NodeId, t: f64| match self.ahead(j, self.grid.index_at(t)) {
                    Some(i) if i < self.len(node) => self.at(node, i),
                    Some(i) => {
                        missing.set(Some((node, i + 1)));
                        f64::INFINITY
                    }
                    None => self.slot(node).before,
                };
                let magnitude = |t: f64| range.from(t, &read).map(|s| s.reach() + s.err);
                if j == 0 {
                    slot.before = magnitude(f64::NEG_INFINITY).unwrap_or(f64::INFINITY);
                }
                let v = magnitude(t);
                match (missing.get(), v, j) {
                    (Some((node, need)), ..) => Point::Needs(node, need),
                    (None, Some(v), _) => Point::Value(v),
                    (None, None, 0) => {
                        Point::Unbounded(self.unknown(id, "a division by what may be zero"))
                    }
                    (None, None, _) => Point::Value(f64::INFINITY),
                }
            }
            Class::Held(source) => {
                let v = self.at(*source, j);
                if j == 0 {
                    slot.before = v;
                }
                Point::Value(v)
            }
            Class::Moved(source, steps) => {
                let v = self.read(*source, self.moved(j, *steps));
                if j == 0 {
                    slot.before = self.slot(*source).before.max(v);
                }
                Point::Value(v)
            }
            Class::Solver(tail) => {
                let q = self.grid.chunk(j);
                match solver::point(tail, q, j == 0) {
                    solver::Found::Value { v, before, ops } => {
                        self.ops += ops;
                        if let Some(before) = before {
                            slot.before = before;
                        }
                        Point::Value(v)
                    }
                    solver::Found::Wait(chunk) => Point::Wait(id, chunk),
                    solver::Found::Unbounded(class) => Point::Unbounded(self.unknown(id, &class)),
                }
            }
            Class::Filter(filtered) => {
                let x = filtered.x;
                if j == 0 {
                    filtered.start(self.slot(x).before);
                    slot.before = filtered.before();
                }
                let at = &self.slot(x).at;
                Point::Value(filtered.at(at, j))
            }
            Class::Loop(looped) => {
                let found = |n: NodeId| match self.slots.get(&n) {
                    Some(Ok(slot)) => Ok(slot.before),
                    Some(Err(e)) => Err(e.clone()),
                    None => Err(self.unknown(n, "a node never bounded")),
                };
                if j == 0 {
                    if let Err(unknown) = looped.start(self.tys(), &found, self.grid.rate) {
                        return Ok(Point::Unbounded(unknown));
                    }
                    slot.before = looped.before();
                }
                let read = |n: NodeId, i: usize| self.at(n, i);
                Point::Value(looped.at(&read, &found, j))
            }
            Class::Op(name, args, numbers) => {
                let rounded = 1.0 + OP * args.len() as f64;
                let values: Vec<f64> = args.iter().map(|a| self.at(*a, j)).collect();
                if j == 0 {
                    let befores: Vec<f64> = args.iter().map(|a| self.slot(*a).before).collect();
                    slot.before = combined(name, &befores, numbers) * rounded;
                }
                Point::Value(combined(name, &values, numbers) * rounded)
            }
            Class::Crop(x, l, r) => {
                let (x, l, r) = (*x, *l, *r);
                let from = |t: f64| match x {
                    Some(x) => self.read(x, self.ahead(j, self.grid.index_at(t.max(l)))),
                    None => f64::INFINITY,
                };
                if j == 0 {
                    slot.before = from(f64::NEG_INFINITY);
                }
                Point::Value(match t >= r {
                    true => 0.0,
                    false => from(t),
                })
            }
        })
    }

    /// A level `id` provably returns to forever from its last instant bounded on, zero
    /// where none is known.
    pub(crate) fn floor(&self, id: NodeId) -> f64 {
        let Some(Ok(slot)) = self.slots.get(&id) else {
            return 0.0;
        };
        match &slot.class {
            Class::Atoms(_, floor) => *floor,
            Class::Held(source) | Class::Moved(source, _) => self.floor(*source),
            Class::Crop(Some(x), _, r) if r.is_infinite() => self.floor(*x),
            Class::Written(form) => {
                let Form::Written(range) = &**form else {
                    unreachable!("a written class holds a written form")
                };
                let last = self.grid.secs(slot.at.len().saturating_sub(1));
                let read = |node: NodeId, t: f64| match self.grid.index_at(t) {
                    Some(i) => self.at(node, i.min(self.len(node).saturating_sub(1))),
                    None => self.slot(node).before,
                };
                let floor_of = |node: NodeId| self.floor(node);
                let reads = Reads {
                    node: &read,
                    floor: &floor_of,
                    rate: self.grid.rate,
                };
                let rounding = range.from(last, &read).map_or(f64::INFINITY, |s| s.err);
                (range.floor(last, &reads) - rounding).max(0.0)
            }
            Class::Op(name, args, numbers) => {
                let floors: Vec<f64> = args.iter().map(|a| self.floor(*a)).collect();
                let tails: Vec<f64> = args.iter().map(|a| self.tail(*a)).collect();
                let constants: f64 = numbers.iter().flatten().map(|k| k.abs()).product();
                let moving: Vec<f64> = numbers
                    .iter()
                    .zip(&floors)
                    .filter(|(k, _)| k.is_none())
                    .map(|(_, f)| *f)
                    .collect();
                let exact = match (name.as_str(), moving.as_slice()) {
                    ("+" | "-", _) => floor::summed(&floors, &tails),
                    ("*", [one]) => one * constants,
                    ("/", _) => floors[0] / numbers[1].map_or(f64::INFINITY, f64::abs),
                    ("pow", _) => floors[0].powi(numbers[1].map_or(0, |n| n as i32)),
                    ("join", _) => floors.iter().copied().fold(0.0, f64::max),
                    ("max" | "min", _) => floor::bounded_away(name, numbers),
                    (other, _) => {
                        Unary::from_name(other).map_or(0.0, |op| floor::through(op, floors[0]))
                    }
                };
                exact * (2.0 - (1.0 + OP * args.len() as f64))
            }
            _ => 0.0,
        }
    }
}

/// Each direct sum's rounding under its factor's supremum from `t` on.
fn direct(form: &Form, t: f64) -> Option<f64> {
    let Form::Atoms(_, summed) = form else {
        return Some(0.0);
    };
    summed.iter().try_fold(0.0, |held, (factor, err)| {
        let under = factor.as_ref().map_or(Some(1.0), |f| sup_from(f, t))?;
        Some(held + err * under)
    })
}

/// A sampled operation over its operands' bounds, before its own rounding.
fn combined(name: &str, values: &[f64], numbers: &[Option<f64>]) -> f64 {
    let joined = |f: fn(f64, f64) -> f64| {
        let mut it = values.iter().copied();
        let first = it.next().expect("an operation with operands");
        it.fold(first, f)
    };
    match name {
        "+" | "-" => joined(|a, b| a + b),
        "*" => joined(|a, b| a * b),
        "/" => values[0] / numbers[1].map_or(f64::NAN, f64::abs),
        "max" | "min" | "join" => joined(f64::max),
        "ch" => values[0],
        "pow" => values[0].powi(numbers[1].map_or(1, |n| n as i32)),
        other => {
            let f = Unary::from_name(other)
                .and_then(mapped)
                .expect("a map checked when built");
            f(values[0])
        }
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

pub(super) fn constant(tys: &Typing, id: NodeId) -> Option<f64> {
    match tys.value(id) {
        Value::ClosedForm(form) if crate::lower::never(&form.body) => Some(f64::INFINITY),
        Value::ClosedForm(form) => crate::lower::constant_value(&form.body, form.var),
        _ => None,
    }
}

pub(super) fn holds_self(tys: &Typing, id: NodeId) -> bool {
    crate::schedule::holds_self(tys, id, &mut BTreeSet::new())
}
