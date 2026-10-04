// Concern: bounds a node's magnitude from an instant on, through what it reads | Non-concern: solvers, loops, where the sound ends | IO: (NodeId) -> a bound from each instant, or none

mod filter;
mod gain;
mod range;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::spectral_sum::sup::magnitude_upper_bound_from_instant;
use sva_formula::{Body, C64, Edge, Fold, Hash, NodeId, Part, SpectralSum, Through, Unary, Var};
use sva_samples::collapse::summed_bounds;
use sva_samples::{
    Audible, CollapseError, Extent, Grid, Profile, truncate_spectral_sum_read,
    truncate_written_with,
};

use crate::cast::Cast;
use crate::lower::number_of;
use crate::refs::read_through;
use crate::typing::{Typing, Value, When};
use filter::Ringing;
pub(super) use gain::{gain, key};
use range::{OP, Range, TRANSFORM_OPS};

pub(super) struct MagnitudeUpperBoundFromInstant {
    form: Form,
    /// A reader reading one node at several places asks it at one instant each time.
    last: Cell<Option<(u64, f64)>>,
}

type Summed = Vec<(Option<SpectralAtom>, f64)>;

enum Form {
    /// Each direct sum over the atoms' lines as its rounding bound and the factor it is under.
    Atoms(Vec<SpectralAtom>, Summed),
    /// Each node the formula reads, bounded by its own form.
    Written(Range, BTreeMap<NodeId, Rc<MagnitudeUpperBoundFromInstant>>),
    /// A fixed filter on its grid's samples, and its input where that never ends.
    Filter(Ringing, Grid, Option<Rc<MagnitudeUpperBoundFromInstant>>),
    /// Every value a draw takes.
    Within(f64),
    /// A read at `k*t + c - step`, `k >= 0`, scaled; any other time reads anywhere.
    Read(Rc<MagnitudeUpperBoundFromInstant>, Map, f64),
}

/// Where a node can be nonzero.
pub(super) type Supported<'a> = &'a dyn Fn(NodeId) -> Extent;

type Key = (Hash, Grid, u32);

#[derive(Default)]
pub(super) struct MagnitudeUpperBoundsFromInstant(
    RefCell<HashMap<Key, Option<Rc<MagnitudeUpperBoundFromInstant>>>>,
);

/// `cut` is whether a bound met a read or a support still open: what it found depends on
/// where the search entered, so it is not kept.
struct Bounding<'a> {
    tys: &'a Typing,
    profile: &'a Profile,
    rate: u32,
    supported: Supported<'a>,
    magnitude_upper_bounds_from_instant: &'a MagnitudeUpperBoundsFromInstant,
    open: BTreeSet<NodeId>,
    cut: bool,
    ordering: bool,
    /// While a node is tried, each read it met unbounded.
    missing: Option<Vec<NodeId>>,
    written: HashMap<NodeId, Range>,
}

impl MagnitudeUpperBoundFromInstant {
    pub(super) fn of(
        tys: &Typing,
        (profile, rate): (&Profile, u32),
        id: NodeId,
        supported: Supported,
        magnitude_upper_bounds_from_instant: &MagnitudeUpperBoundsFromInstant,
    ) -> Option<Rc<MagnitudeUpperBoundFromInstant>> {
        let mut bounding = Bounding {
            tys,
            profile,
            rate,
            supported,
            magnitude_upper_bounds_from_instant,
            open: BTreeSet::new(),
            cut: false,
            ordering: false,
            missing: None,
            written: HashMap::new(),
        };
        bounding.magnitude_upper_bound_from_instant(id)
    }

    fn new(form: Form) -> Option<Rc<MagnitudeUpperBoundFromInstant>> {
        Some(Rc::new(MagnitudeUpperBoundFromInstant {
            form,
            last: Cell::new(None),
        }))
    }
}

impl Bounding<'_> {
    fn key(&self, id: NodeId) -> Option<Key> {
        let grid = self.tys.grid(id);
        crate::refs::identity(self.tys, id)
            .ok()
            .map(|held| (held, grid, self.rate))
    }

    fn held(&self, id: NodeId) -> Option<Option<Rc<MagnitudeUpperBoundFromInstant>>> {
        let key = self.key(id)?;
        self.magnitude_upper_bounds_from_instant
            .0
            .borrow()
            .get(&key)
            .cloned()
    }

    fn magnitude_upper_bound_from_instant(
        &mut self,
        id: NodeId,
    ) -> Option<Rc<MagnitudeUpperBoundFromInstant>> {
        if let Some(held) = self.held(id) {
            return held;
        }
        if let Some(missing) = &mut self.missing {
            missing.push(id);
            return None;
        }
        if !std::mem::replace(&mut self.ordering, true) {
            self.order(id);
            if let Some(held) = self.held(id) {
                return held;
            }
        }
        self.bounded(id)
    }

    fn bounded(&mut self, id: NodeId) -> Option<Rc<MagnitudeUpperBoundFromInstant>> {
        crate::steps::step(1);
        let outer = std::mem::take(&mut self.cut);
        let before = self.missing.as_ref().map(Vec::len);
        let found = self.fresh(id);
        let whole = self.missing.as_ref().map(Vec::len) == before;
        if let Some(key) = self.key(id)
            && !self.cut
            && whole
        {
            self.magnitude_upper_bounds_from_instant
                .0
                .borrow_mut()
                .insert(key, found.clone());
        }
        self.cut |= outer;
        found
    }

    /// Bounds what `root`'s bound reads, readers after what they read, so bounding `root`
    /// recurses one read deep; a try meeting a read unbounded is retried after it.
    fn order(&mut self, root: NodeId) {
        let mut seen = BTreeSet::new();
        let mut open = vec![(root, false)];
        while let Some((id, met)) = open.pop() {
            if self.held(id).is_some() {
                continue;
            }
            match met {
                true => {
                    self.bounded(id);
                }
                false if seen.insert(id) => {
                    let cut = self.cut;
                    self.missing = Some(Vec::new());
                    self.bounded(id);
                    let missing = self.missing.take().unwrap_or_default();
                    self.cut = cut;
                    if !missing.is_empty() {
                        open.push((id, true));
                        open.extend(missing.into_iter().rev().map(|n| (n, false)));
                    }
                }
                false => {}
            }
        }
    }

    fn opened(&mut self, id: NodeId) -> bool {
        let opened = self.open.insert(id);
        self.cut |= !opened;
        opened
    }

    fn fresh(&mut self, id: NodeId) -> Option<Rc<MagnitudeUpperBoundFromInstant>> {
        if let Some(range) = self.written.remove(&id) {
            return self.written(id, range);
        }
        let (tys, profile, rate) = (self.tys, self.profile, self.rate);
        // A retired term sounded before now, in what reads the note sum and in no bound here.
        if tys.retired_sum().is_some_and(|sum| tys.reads(id, sum)) {
            return None;
        }
        let band = Audible::of(profile, rate);
        // A series no line reaches falls to the written form, as the collapse does.
        let whole = match tys.ty(id).is_closed_form() {
            true => crate::refs::spectral_sum_of(tys, id, Var::T).ok(),
            false => None,
        };
        if let Some(whole) = whole
            && let Ok(sum) = read_through(tys, |t| truncate_spectral_sum_read(&whole, band, t))
        {
            let summed = read_through(tys, |t| summed_bounds(&whole, (profile, rate), t));
            return MagnitudeUpperBoundFromInstant::new(Form::Atoms(atoms_of(&sum), summed.ok()?));
        }
        let body = match tys.value(id) {
            Value::ClosedForm(form) if form.var == Var::T => {
                read_through(tys, |t| {
                    truncate_written_with(&form.body, band, t, &mut |id, _| match Through::stands(
                        id,
                    ) {
                        true => Err(CollapseError::NotEvaluable(
                            "a ref read at a term's own time",
                        )),
                        false => Ok(id),
                    })
                })
                .ok()?
            }
            Value::Cast(Cast::Sample, source) => {
                return self.magnitude_upper_bound_from_instant(*source);
            }
            Value::Noise(_) => return MagnitudeUpperBoundFromInstant::new(Form::Within(1.0)),
            Value::Op { name, args } => sampled(tys, name, args)?,
            Value::Read { .. } if let Some(source) = crate::refs::passes(tys, id) => {
                return self.magnitude_upper_bound_from_instant(source);
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
                let inner = self.magnitude_upper_bound_from_instant(*source);
                self.open.remove(&id);
                let inner = inner?;
                return MagnitudeUpperBoundFromInstant::new(match &inner.form {
                    Form::Read(foot, under, gain) if let Some(map) = composed(read, *under) => {
                        Form::Read(Rc::clone(foot), map, *gain)
                    }
                    _ => Form::Read(inner, read, 1.0),
                });
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
                let input = self.magnitude_upper_bound_from_instant(*x)?;
                let span = (self.supported)(*x);
                let first = match span.start {
                    i64::MIN => f64::NEG_INFINITY,
                    start => grid.instant(start),
                };
                let ringing = Ringing::of(&coeffs, input.at_and_after_instant(first), span.end)?;
                let endless = (span.end == i64::MAX).then_some(input);
                return MagnitudeUpperBoundFromInstant::new(Form::Filter(ringing, grid, endless));
            }
            _ => return None,
        };
        let range = Range::of(&body).ok()?;
        self.written(id, range)
    }

    /// A try keeps the form for its retry.
    fn written(&mut self, id: NodeId, range: Range) -> Option<Rc<MagnitudeUpperBoundFromInstant>> {
        let mut nodes = Vec::new();
        range.nodes(&mut nodes);
        self.opened(id).then_some(())?;
        let mut reads = BTreeMap::new();
        let mut whole = true;
        for n in nodes {
            let before = self.missing.as_ref().map(Vec::len);
            match self.magnitude_upper_bound_from_instant(n) {
                Some(upper_bound) => {
                    reads.insert(n, upper_bound);
                }
                None if self.missing.as_ref().map(Vec::len) != before => whole = false,
                None => {
                    self.open.remove(&id);
                    return None;
                }
            }
        }
        self.open.remove(&id);
        if !whole {
            self.written.insert(id, range);
            return None;
        }
        MagnitudeUpperBoundFromInstant::new(
            linear(&range, &reads).unwrap_or(Form::Written(range, reads)),
        )
    }
}

impl MagnitudeUpperBoundFromInstant {
    /// Bounds `|x(s)|` for every `s >= t`, its rounding included; infinite where none holds,
    /// never NaN.
    /// Each bound it reads is bounded first, at each instant it asks, so a chain of reads costs
    /// heap, never call depth.
    pub(super) fn at_and_after_instant<'a>(&'a self, t: f64) -> f64 {
        if let Some((held, bound)) = self.last.get()
            && held == t.to_bits()
        {
            return bound;
        }
        let mut found: HashMap<(*const MagnitudeUpperBoundFromInstant, u64), f64> = HashMap::new();
        let mut open: Vec<(&'a MagnitudeUpperBoundFromInstant, f64)> = vec![(self, t)];
        while let Some(&(upper_bound, at)) = open.last() {
            let key = (std::ptr::from_ref(upper_bound), at.to_bits());
            if found.contains_key(&key) {
                open.pop();
                continue;
            }
            if let Some((held, bound)) = upper_bound.last.get()
                && held == at.to_bits()
            {
                found.insert(key, bound);
                open.pop();
                continue;
            }
            let missing: RefCell<Vec<(&'a MagnitudeUpperBoundFromInstant, f64)>> =
                RefCell::default();
            let read = |read: &'a Rc<MagnitudeUpperBoundFromInstant>, at: f64| -> f64 {
                let read: &'a MagnitudeUpperBoundFromInstant = read;
                if let Some((held, bound)) = read.last.get()
                    && held == at.to_bits()
                {
                    return bound;
                }
                match found.get(&(std::ptr::from_ref(read), at.to_bits())) {
                    Some(bound) => *bound,
                    None => {
                        missing.borrow_mut().push((read, at));
                        f64::INFINITY
                    }
                }
            };
            crate::steps::step(1);
            // A bound no arithmetic defines proves nothing.
            let bound = match upper_bound.bound_from(at, &read) {
                b if b.is_nan() => f64::INFINITY,
                b => b,
            };
            let missing = missing.into_inner();
            if missing.is_empty() {
                upper_bound.last.set(Some((at.to_bits(), bound)));
                found.insert(key, bound);
                open.pop();
            } else {
                open.extend(missing.into_iter().rev());
            }
        }
        found[&(std::ptr::from_ref(self), t.to_bits())]
    }

    /// From the double before `t`: an edge tying `t` may hold it.
    fn bound_from<'a>(
        &'a self,
        t: f64,
        read: &dyn Fn(&'a Rc<MagnitudeUpperBoundFromInstant>, f64) -> f64,
    ) -> f64 {
        let t = t.next_down();
        match &self.form {
            Form::Atoms(atoms, summed) => {
                atoms_magnitude_upper_bound_from_instant(atoms, summed, t)
            }
            Form::Written(range, reads) => range
                .from(t, &|n, t| read(&reads[&n], t))
                .map_or(f64::INFINITY, |s| s.reach() + s.err),
            Form::Filter(ringing, grid, None) => ringing.from(sample(*grid, t)),
            Form::Filter(ringing, grid, Some(input)) => {
                let n0 = sample(*grid, t).saturating_sub(ringing.settle().saturating_add(2));
                ringing.fed_from(read(input, grid.instant(n0)))
            }
            Form::Within(m) => *m,
            Form::Read(source, Some((k, c, step)), gain) if *k >= 0.0 => {
                times(*gain, read(source, k * t + c - step))
            }
            Form::Read(source, _, gain) => times(*gain, read(source, f64::NEG_INFINITY)),
        }
    }
}

type Map = Option<(f64, f64, f64)>;

fn sample(grid: Grid, t: f64) -> i64 {
    grid.count(t).floor().clamp(-9e18, 9e18) as i64
}

fn atoms_magnitude_upper_bound_from_instant(
    atoms: &[SpectralAtom],
    summed: &Summed,
    t: f64,
) -> f64 {
    let rounded = 1.0 + OP * (atoms.len() as f64 + TRANSFORM_OPS);
    let mut sum = 0.0;
    for atom in atoms {
        match magnitude_upper_bound_from_instant(atom, t) {
            Some(sup) => sum += sup,
            None => return f64::INFINITY,
        }
    }
    let direct = summed.iter().try_fold(0.0, |held, (factor, err)| {
        let under = factor
            .as_ref()
            .map_or(Some(1.0), |f| magnitude_upper_bound_from_instant(f, t))?;
        Some(held + err * under)
    });
    direct.map_or(f64::INFINITY, |direct| sum * rounded + direct)
}

/// Each atom a truncated sum holds, its modal banks' included.
fn atoms_of(sum: &SpectralSum) -> Vec<SpectralAtom> {
    let atoms: Vec<SpectralAtom> = sum
        .lanes
        .iter()
        .flat_map(|lane| {
            let modal = lane
                .modal
                .iter()
                .flat_map(|bank| sva_formula::modal::atoms(bank, sva_formula::Origin::UNKNOWN));
            lane.atoms.iter().copied().chain(modal)
        })
        .collect();
    crate::steps::step(atoms.len());
    atoms
}

fn times(gain: f64, bound: f64) -> f64 {
    match gain == 0.0 {
        true => 0.0,
        false => gain * bound,
    }
}

/// A form linear in reads of one foot on one clock, as one scaled read of it where any reads
/// earliest: bounds never rise with their instant, and the span is homogeneous in its reads.
fn linear(
    range: &Range,
    reads: &BTreeMap<NodeId, Rc<MagnitudeUpperBoundFromInstant>>,
) -> Option<Form> {
    if reads.is_empty() || !range.linear() {
        return None;
    }
    let identity = Some((1.0, 0.0, 0.0));
    let feet = reads.values().map(|upper_bound| match &upper_bound.form {
        Form::Read(foot, map, gain) => (foot, *map, *gain),
        _ => (upper_bound, identity, 1.0),
    });
    let feet: Vec<(&Rc<MagnitudeUpperBoundFromInstant>, Map, f64)> = feet.collect();
    let (foot, first, _) = feet[0];
    let (k, ..) = first?;
    let mut earliest = f64::INFINITY;
    for (other, map, _) in &feet {
        let (k_n, c, step) = (*map)?;
        if !Rc::ptr_eq(other, foot) || k_n != k || k < 0.0 {
            return None;
        }
        earliest = earliest.min(c - step);
    }
    let gains: BTreeMap<NodeId, f64> = reads.keys().zip(&feet).map(|(n, f)| (*n, f.2)).collect();
    let span = range.from(f64::NEG_INFINITY, &|n, _| gains[&n])?;
    let gain = (span.reach() + span.err) * (1.0 + OP);
    gain.is_finite()
        .then(|| Form::Read(Rc::clone(foot), Some((k, earliest, 0.0)), gain))
}

/// A read of a read as one read of what the inner one reads, so a chain of them bounds in one
/// step; `None` where the two do not compose. A map reading anywhere, or backwards, is `None`.
fn composed(outer: Map, inner: Map) -> Option<Map> {
    let forward = |map: Map| map.filter(|(k, ..)| *k >= 0.0);
    match (forward(outer), forward(inner)) {
        (_, None) => Some(None),
        (None, Some((k, ..))) => (k > 0.0).then_some(None),
        (Some((k1, c1, s1)), Some((k2, c2, s2))) => {
            Some(Some((k2 * k1, k2 * c1 + c2, k2 * s1 + s2)))
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

#[cfg(test)]
mod tests {
    use super::{Map, composed};
    use crate::RenderConfig;

    fn ending(files: &[(String, String)], root: &str) -> u64 {
        let mut composition = sva_ast::Composition::new();
        for (name, body) in files {
            composition.insert(name.as_str(), body.as_str());
        }
        let g = sva_ast::load(&composition).expect("a composition");
        let before = crate::steps::taken();
        crate::render::ends(&g, &[root.to_string()], &RenderConfig::at(48_000)).expect("an end");
        crate::steps::taken() - before
    }

    /// A noise series through `depth` filters, each a form of its own, under a falling window.
    fn breath(depth: usize) -> Vec<(String, String)> {
        let mut x = "noise(9, period=8/185, color=-3)".to_string();
        for k in 0..depth {
            x = format!("lowpass({x}, {}, 0.7)", 11_000 - 100 * k);
        }
        vec![("w".to_string(), format!("crop({x}, 0s, 0.5s, fall=0.3s)\n"))]
    }

    /// Only the form the window reads is bounded, not each filter's: no filter adds a step.
    #[test]
    fn a_series_through_filters_is_bounded_once_however_many_it_passes() {
        let (one, six) = (ending(&breath(1), "w"), ending(&breath(6), "w"));
        assert_eq!(six, one);
    }

    fn chain(depth: usize) -> Vec<(String, String)> {
        let bodies = [
            "0.999*@P(t)",
            "max(@P(t), -1)",
            "crop(lowpass(sample(@P(t)), 3000), 0s, 1s)",
            "@P(t - 1ms)",
            "tanh(@P(t))",
        ];
        let mut files = vec![(
            "c0".to_string(),
            "crop(sin(2*pi*220*t), 0s, 0.1s)\n".to_string(),
        )];
        for k in 1..=depth {
            let body = bodies[k % bodies.len()].replace('P', &format!("c{}", k - 1));
            files.push((format!("c{k}"), format!("{body}\n")));
        }
        files.push(("top".to_string(), format!("crop(@c{depth}(t), 0s, 1s)\n")));
        files
    }

    /// Four times the refs, under twice four times the steps: a fold per node grows as the
    /// chain does, where a walk per path would grow sixteenfold.
    #[test]
    fn a_chains_supports_and_bounds_take_steps_linear_in_its_refs() {
        let (short, long) = (ending(&chain(250), "top"), ending(&chain(1000), "top"));
        assert!(long < 8 * short, "{short} then {long}");
    }

    /// Where a map reads at `t`; anywhere is `-inf`, as a bound reads it.
    fn at(map: Map, t: f64) -> f64 {
        match map {
            Some((k, c, s)) if k >= 0.0 => k * t + c - s,
            _ => f64::NEG_INFINITY,
        }
    }

    #[test]
    fn a_read_of_a_read_reads_where_the_two_in_turn_do() {
        let maps: [Map; 6] = [
            Some((1.0, -0.01, 1.0 / 8_000.0)),
            Some((2.0, 0.25, 1.0 / 48_000.0)),
            Some((0.5, 3.0, 1.0 / 44_100.0)),
            Some((0.0, 1.5, 1.0 / 8_000.0)),
            Some((-1.0, 0.0, 1.0 / 8_000.0)),
            None,
        ];
        for outer in maps {
            for inner in maps {
                let Some(one) = composed(outer, inner) else {
                    let nowhere = at(inner, at(outer, 0.0)).is_nan();
                    assert!(nowhere, "{outer:?} over {inner:?} composes");
                    continue;
                };
                for t in [-2.0, 0.0, 0.37, 5.0] {
                    let (two, one) = (at(inner, at(outer, t)), at(one, t));
                    let near = (two - one).abs() <= 1e-12 * two.abs().max(1.0);
                    assert!(
                        two == one || near,
                        "{outer:?} {inner:?} at {t}: {two} {one}"
                    );
                }
            }
        }
    }
}
