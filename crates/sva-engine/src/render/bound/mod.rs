// Concern: bounds a node's magnitude from an instant on: form, operation, read, fixed filter | Non-concern: solvers, loops, gain to the output | IO: (NodeId) -> a bound from each instant, or none

mod filter;
mod range;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::spectral_sum::sup::sup_from;
use sva_formula::{Body, C64, Edge, Fold, Hash, NodeId, Part, Through, Unary, Var};
use sva_samples::collapse::plan::summed_bounds;
use sva_samples::{
    Audible, CollapseError, Extent, Grid, Profile, truncate_spectral_sum_read,
    truncate_written_with,
};

use crate::cast::Cast;
use crate::lower::number_of;
use crate::refs::read_through;
use crate::typing::{Typing, Value, When};
use filter::Ringing;
use range::{OP, Range, TRANSFORM_OPS};

pub(crate) struct Tail {
    form: Form,
    /// A reader reading one node at several places asks it at one instant each time.
    last: Cell<Option<(u64, f64)>>,
}

enum Form {
    /// Each direct sum over the atoms' lines as its rounding bound and the factor it is under.
    Atoms(Vec<SpectralAtom>, Vec<(Option<SpectralAtom>, f64)>),
    /// Each node the formula reads, bounded by its own form.
    Written(Range, BTreeMap<NodeId, Rc<Tail>>),
    /// A fixed filter on its grid's samples.
    Filter(Ringing, Grid),
    /// Every value a draw takes.
    Within(f64),
    /// A read at `k*t + c`, `k >= 0`, at most `step` early; any other time reads anywhere.
    Read(Rc<Tail>, Option<(f64, f64, f64)>),
}

/// Where a node is nonzero, its prune included; `false` where it is still being found.
pub(crate) type Ends<'a> = &'a dyn Fn(NodeId) -> (Extent, bool);

type Key = (Hash, Grid, u32);

#[derive(Default)]
pub(crate) struct Tails(RefCell<HashMap<Key, Option<Rc<Tail>>>>);

/// `cut` is whether a bound met a read or a support still open: what it found depends on
/// where the search entered, so it is not kept.
struct Bounding<'a> {
    tys: &'a Typing,
    profile: &'a Profile,
    rate: u32,
    ends: Ends<'a>,
    tails: &'a Tails,
    open: BTreeSet<NodeId>,
    cut: bool,
}

impl Tail {
    pub(crate) fn of(
        tys: &Typing,
        (profile, rate): (&Profile, u32),
        id: NodeId,
        ends: Ends,
        tails: &Tails,
    ) -> Option<Rc<Tail>> {
        let mut bounding = Bounding {
            tys,
            profile,
            rate,
            ends,
            tails,
            open: BTreeSet::new(),
            cut: false,
        };
        bounding.tail(id)
    }

    fn new(form: Form) -> Option<Rc<Tail>> {
        Some(Rc::new(Tail {
            form,
            last: Cell::new(None),
        }))
    }
}

impl Bounding<'_> {
    fn tail(&mut self, id: NodeId) -> Option<Rc<Tail>> {
        let grid = self.tys.grid(id);
        let key = crate::refs::identity(self.tys, id)
            .ok()
            .map(|held| (held, grid, self.rate));
        if let Some(held) = key.and_then(|key| self.tails.0.borrow().get(&key).cloned()) {
            return held;
        }
        let outer = std::mem::take(&mut self.cut);
        let found = self.fresh(id);
        if let Some(key) = key
            && !self.cut
        {
            self.tails.0.borrow_mut().insert(key, found.clone());
        }
        self.cut |= outer;
        found
    }

    fn opened(&mut self, id: NodeId) -> bool {
        let opened = self.open.insert(id);
        self.cut |= !opened;
        opened
    }

    fn fresh(&mut self, id: NodeId) -> Option<Rc<Tail>> {
        let (tys, profile, rate) = (self.tys, self.profile, self.rate);
        // A retired term sounded before now, in what reads the note sum and in no bound here.
        if tys.retired_sum().is_some_and(|sum| tys.reads(id, sum)) {
            return None;
        }
        let band = Audible::of(profile, rate);
        // A series no line reaches falls to the written form, as the collapse does.
        if tys.ty(id).is_closed_form()
            && let Ok(whole) = crate::refs::spectral_sum_of(tys, id, Var::T)
            && let Ok(sum) = read_through(tys, |t| truncate_spectral_sum_read(&whole, band, t))
        {
            let summed = read_through(tys, |t| summed_bounds(&whole, (profile, rate), t)).ok()?;
            let atoms = sum
                .lanes
                .iter()
                .flat_map(|lane| {
                    let modal = lane.modal.iter().flat_map(|bank| {
                        sva_formula::modal::atoms(bank, sva_formula::Origin::UNKNOWN)
                    });
                    lane.atoms.iter().copied().chain(modal)
                })
                .collect();
            return Tail::new(Form::Atoms(atoms, summed));
        }
        let body =
            match tys.value(id) {
                Value::ClosedForm(form) if form.var == Var::T => read_through(tys, |t| {
                    truncate_written_with(&form.body, band, t, &mut |id, _| match Through::stands(
                        id,
                    ) {
                        true => Err(CollapseError::NotEvaluable(
                            "a ref read at a term's own time",
                        )),
                        false => Ok(id),
                    })
                })
                .ok()?,
                Value::Cast(Cast::Sample, source) => return self.tail(*source),
                Value::Noise(_) => return Tail::new(Form::Within(1.0)),
                Value::Op { name, args } => sampled(tys, name, args)?,
                Value::Read { .. } if let Some(source) = crate::refs::passes(tys, id) => {
                    return self.tail(source);
                }
                Value::Read { source, at, .. } => {
                    let read = match at {
                        When::At(map) => {
                            let step = 1.0 / tys.grid(*source).sr();
                            Some((map.scale.to_f64(), map.shift.to_f64(), step))
                        }
                        _ => None,
                    };
                    self.opened(id).then_some(())?;
                    let inner = self.tail(*source);
                    self.open.remove(&id);
                    return Tail::new(Form::Read(inner?, read));
                }
                Value::Filter {
                    shape,
                    x,
                    cutoff,
                    q,
                    gain,
                } => {
                    let [cutoff, q, gain] = [cutoff, q, gain].map(|p| number_of(tys, *p));
                    let grid = tys.grid(id);
                    if tys.grid(*x) != grid {
                        return None;
                    }
                    let (coeffs, _) =
                        sva_samples::filters::coefficients(*shape, cutoff?, q?, gain?, grid.sr());
                    let input = self.tail(*x)?;
                    let (span, found) = (self.ends)(*x);
                    self.cut |= !found;
                    let first = match span.start {
                        i64::MIN => f64::NEG_INFINITY,
                        start => grid.instant(start),
                    };
                    let ringing = Ringing::of(&coeffs, input.from(first), span.end)?;
                    return Tail::new(Form::Filter(ringing, grid));
                }
                _ => return None,
            };
        let range = Range::of(&body).ok()?;
        let mut nodes = Vec::new();
        range.nodes(&mut nodes);
        self.opened(id).then_some(())?;
        let reads = nodes
            .into_iter()
            .map(|n| Some((n, self.tail(n)?)))
            .collect::<Option<BTreeMap<_, _>>>();
        self.open.remove(&id);
        Tail::new(Form::Written(range, reads?))
    }
}

impl Tail {
    /// Bounds `|x(s)|` for every `s >= t`, its rounding included; infinite where none holds.
    pub(crate) fn from(&self, t: f64) -> f64 {
        if let Some((at, bound)) = self.last.get()
            && at == t.to_bits()
        {
            return bound;
        }
        let bound = self.bound_from(t);
        self.last.set(Some((t.to_bits(), bound)));
        bound
    }

    fn bound_from(&self, t: f64) -> f64 {
        match &self.form {
            Form::Atoms(atoms, summed) => {
                let rounded = 1.0 + OP * (atoms.len() as f64 + TRANSFORM_OPS);
                let mut sum = 0.0;
                for atom in atoms {
                    match sup_from(atom, t) {
                        Some(sup) => sum += sup,
                        None => return f64::INFINITY,
                    }
                }
                let direct = summed.iter().try_fold(0.0, |held, (factor, err)| {
                    let under = factor.as_ref().map_or(Some(1.0), |f| sup_from(f, t))?;
                    Some(held + err * under)
                });
                direct.map_or(f64::INFINITY, |direct| sum * rounded + direct)
            }
            Form::Written(range, reads) => range
                .from(t, &|n, t| reads[&n].from(t))
                .map_or(f64::INFINITY, |s| s.reach() + s.err),
            Form::Filter(ringing, grid) => {
                ringing.from(grid.count(t).floor().clamp(-9e18, 9e18) as i64)
            }
            Form::Within(m) => *m,
            Form::Read(source, Some((k, c, step))) if *k >= 0.0 => source.from(k * t + c - step),
            Form::Read(source, _) => source.from(f64::NEG_INFINITY),
        }
    }
}

/// A sampled operation as the written form its value is, each operand a node and each constant
/// its number; `None` for `%` and a power no constant names.
fn sampled(tys: &Typing, name: &str, args: &[NodeId]) -> Option<Body> {
    let number = |at: usize| args.get(at).and_then(|a| number_of(tys, *a));
    let part = |a: &NodeId| {
        Part::bare(match number_of(tys, *a) {
            Some(c) => Body::Const(C64::real(c)),
            None => Body::Node(*a),
        })
    };
    let parts = || args.iter().map(part).collect::<Vec<_>>();
    let pair = || Some((part(args.first()?), part(args.get(1)?)));
    Some(match name {
        "+" => Body::Add(parts()),
        "*" => Body::Mul(parts()),
        "-" => {
            let (a, b) = pair()?;
            let minus = Part::bare(Body::Mul(vec![Part::bare(Body::Const(C64::real(-1.0))), b]));
            Body::Add(vec![a, minus])
        }
        "/" => {
            let (a, b) = pair()?;
            Body::Div(a, b)
        }
        "pow" => {
            let n = number(1).filter(|n| n.fract() == 0.0 && n.abs() < 64.0)?;
            Body::Pow(part(args.first()?), n as i32)
        }
        "max" => Body::Fold(Fold::Max, parts()),
        "min" => Body::Fold(Fold::Min, parts()),
        // Where the window shuts is the node's support; its gain is at most one.
        "crop" => Body::Crop {
            of: part(args.first()?),
            l: Edge::at(f64::NEG_INFINITY),
            r: Edge::at(f64::INFINITY),
            rise: 0.0,
            fall: 0.0,
        },
        "join" => Body::Join(parts()),
        "ch" => {
            let k = number(1).filter(|k| k.fract() == 0.0 && (0.0..256.0).contains(k))?;
            Body::Channel(part(args.first()?), k as u8)
        }
        other => Body::Apply(Unary::from_name(other)?, part(args.first()?)),
    })
}
