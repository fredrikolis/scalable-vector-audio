// Concern: truncates every series to the terms the profile leaves, once per collapse | Non-concern: evaluating what is left (point.rs) | IO: (&SpectralSum or &Body) -> the same, series-free

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::TAU;

use sva_formula::affine::{Axis, axis_read, exact_constant_at, exact_constant_read};
use sva_formula::closed_form::{Bound, Series, children, map_children};
use sva_formula::fourier_dual::series::{Shape, read_with};
use sva_formula::series::{falls, mentions, mentions_line_read, ratio, substitute};
use sva_formula::spectral_sum::atom::{Exp, Factors, Singular, SpectralAtom};
use sva_formula::spectral_sum::merge::simplify;
use sva_formula::spectral_sum::sup::sup_from;
use sva_formula::through::{Read, looked};
use sva_formula::{
    Banded, Body, C64, Codomain, Env, IndexId, Lane, NodeId, Opaque, ParamId, Part, Reads, Run,
    SpectralSum, Ty, Unary, Var, d_dt, lines_read, normalize_read,
};

use crate::error::CollapseError;
use crate::profile::Profile;

/// What a series is truncated against: the observation's own ceiling, the amplitude a
/// dropped tail provably stays within, and the floor a tail no bound sums falls under.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Audible {
    ceiling: f64,
    precision: f64,
    floor_db: f64,
}

impl Audible {
    pub fn of(profile: &Profile, rate: u32) -> Audible {
        Audible::on(profile, crate::grid::Grid::of(rate))
    }

    pub fn on(profile: &Profile, grid: crate::grid::Grid) -> Audible {
        let ceiling = profile.ceiling_hz.min(grid.sr() / 2.0);
        Audible {
            ceiling,
            precision: profile.half_lsb(),
            floor_db: profile.floor(ceiling),
        }
    }

    pub(crate) fn ceiling(self) -> f64 {
        self.ceiling
    }

    pub fn key(self) -> [u64; 3] {
        [self.ceiling, self.precision, self.floor_db].map(f64::to_bits)
    }

    /// A line read at `at(t)` sounds at `hz*at'(t)`; an unbounded `at'` keeps the band, unclaimed.
    fn read_at(self, at: &Body, reads: &dyn Reads) -> Audible {
        match sva_formula::calculus::steepest_read(at, reads) {
            Some(rate) if rate > 1.0 => Audible {
                ceiling: self.ceiling / rate,
                ..self
            },
            _ => self,
        }
    }
}

/// A line series is placed analytically under `sva_formula`'s own bound; an expanded one
/// becomes a formula the sample loop walks, which is what this caps.
const MAX_EXPANDED_TERMS: usize = 1 << 13;

/// The most terms of a carrier series one instant sums: every term under a 20 kHz ceiling
/// down to a 19.5 Hz fundamental, the bottom of the audible band; below it what an instant
/// leaves out is bounded and reported.
const MOST_PER_INSTANT: i64 = 1 << 10;

/// FORMAT 6.2 truncates a series once, at collapse: every term the ceiling and the precision
/// leave becomes an ordinary atom before the first sample is read.
pub fn spectral_sum(n: &SpectralSum, band: Audible) -> Result<SpectralSum, CollapseError> {
    spectral_sum_read(n, band, &Opaque)
}

pub fn spectral_sum_read(
    n: &SpectralSum,
    band: Audible,
    reads: &dyn Reads,
) -> Result<SpectralSum, CollapseError> {
    Terms::new(reads).spectral_sum(n, band)
}

/// The same truncation over a written closed form, whose series a spectral sum never reached.
pub fn written(f: &Body, band: Audible) -> Result<Body, CollapseError> {
    written_with(f, band, &Opaque, &mut |id, _| Ok(id))
}

/// Each ref truncated as the form `reads` names, under the band it is read in, and renamed to
/// what `named` calls that truncation.
pub fn written_with(
    f: &Body,
    band: Audible,
    reads: &dyn Reads,
    named: &mut dyn FnMut(NodeId, Audible) -> Result<NodeId, CollapseError>,
) -> Result<Body, CollapseError> {
    Terms::new(reads).written(f, band, named)
}

/// The loudest term any banded series in `f` may drop, in dB against its loudest.
pub fn dropped_db(f: &Body) -> Option<f64> {
    let own = match f {
        Body::Banded(b) => Some(b.dropped_db),
        _ => None,
    };
    let inner = children(f).into_iter().filter_map(|p| dropped_db(&p.body));
    own.into_iter().chain(inner).reduce(f64::max)
}

pub(super) fn windowed_lines(
    s: &Series,
    band: Audible,
    reads: &dyn Reads,
) -> Option<(Vec<sva_formula::Line>, Option<sva_formula::Indicator>)> {
    Terms::new(reads).windowed_lines(s, band)
}

/// One truncation's reading of the refs its terms hold, once per node and band.
struct Terms<'a> {
    reads: &'a dyn Reads,
    bounds: PerBand<Option<Bounded>>,
    prices: PerBand<Option<Price>>,
    series: RefCell<HashMap<NodeId, bool>>,
    summed: PerBand<NodeId>,
}

type PerBand<T> = RefCell<HashMap<(NodeId, [u64; 3]), T>>;

/// Series deep, and terms the whole nesting takes.
type Price = (usize, Option<usize>);

/// Series deep, terms the whole nesting takes, terms this level takes.
struct Cost {
    depth: usize,
    terms: Option<usize>,
    own: usize,
}

/// A term's coefficient in the index. `exact` where it bounds the term's magnitude; elsewhere
/// it only stands in, a turning factor or a name read as one.
#[derive(Clone)]
struct Bounded {
    body: Body,
    exact: bool,
}

impl<'a> Terms<'a> {
    fn new(reads: &'a dyn Reads) -> Terms<'a> {
        Terms {
            reads,
            bounds: RefCell::default(),
            prices: RefCell::default(),
            series: RefCell::default(),
            summed: RefCell::default(),
        }
    }

    fn holds_series(&self, id: NodeId) -> bool {
        if let Some(held) = self.series.borrow().get(&id) {
            return *held;
        }
        let form = self.reads.view(id, &Read::As);
        let found = form.is_some_and(|form| self.reaches_series(&form));
        self.series.borrow_mut().insert(id, found);
        found
    }

    fn reaches_series(&self, f: &Body) -> bool {
        match f {
            Body::Series(_) => true,
            Body::Node(id) => self.holds_series(*id),
            other => children(other).iter().any(|p| self.reaches_series(&p.body)),
        }
    }

    /// `id`, or a node standing for its form with each series it holds summed term by term.
    fn summed(&self, id: NodeId, band: Audible) -> Result<NodeId, CollapseError> {
        if !self.holds_series(id) {
            return Ok(id);
        }
        let key = (id, band.key());
        if let Some(held) = self.summed.borrow().get(&key) {
            return Ok(*held);
        }
        let form = self
            .reads
            .view(id, &Read::As)
            .expect("a ref holding a series names a form");
        let body = self.written(&form, band, &mut |n, b| self.summed(n, b))?;
        let stand = self
            .reads
            .stand_for(body)
            .expect("a reader handing out forms stands one in");
        self.summed.borrow_mut().insert(key, stand);
        Ok(stand)
    }

    fn spectral_sum(&self, n: &SpectralSum, band: Audible) -> Result<SpectralSum, CollapseError> {
        if n.lanes.iter().all(|l| l.series.is_empty()) {
            return Ok(n.clone());
        }
        let mut lanes = Vec::with_capacity(n.lanes.len());
        for lane in &n.lanes {
            let mut held = Lane {
                series: Vec::new(),
                ..lane.clone()
            };
            for s in &lane.series {
                held.atoms.extend(self.atoms(s, band)?);
            }
            lanes.push(held);
        }
        Ok(SpectralSum::of(n.var, lanes))
    }

    fn written(
        &self,
        f: &Body,
        band: Audible,
        named: &mut dyn FnMut(NodeId, Audible) -> Result<NodeId, CollapseError>,
    ) -> Result<Body, CollapseError> {
        match f {
            Body::Node(id) => return Ok(Body::Node(named(*id, band)?)),
            Body::Series(s) => return self.expanded(s, band, named),
            Body::Warp { at, of } => {
                let moved = band.read_at(&at.body, self.reads);
                return Ok(Body::Warp {
                    at: Part::new(at.origin, self.written(&at.body, band, named)?),
                    of: Part::new(of.origin, self.written(&of.body, moved, named)?),
                });
            }
            _ => {}
        }
        let mut found = None;
        let out = map_children(f, |p| match self.written(&p.body, band, named) {
            Ok(body) => Part::new(p.origin, body),
            Err(e) => {
                found = Some(e);
                p.clone()
            }
        });
        match found {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    fn windowed_lines(
        &self,
        s: &Series,
        band: Audible,
    ) -> Option<(Vec<sva_formula::Line>, Option<sva_formula::Indicator>)> {
        let (body, window) = sva_formula::crop_peeled(&looked(&s.term.body, self.reads));
        let bare = Series {
            term: Part::new(s.term.origin, body),
            ..s.clone()
        };
        let taken = self.enumerated(&bare, band).ok()?;
        (!taken.is_empty()).then_some((taken, window))
    }

    fn atoms(&self, s: &Series, band: Audible) -> Result<Vec<SpectralAtom>, CollapseError> {
        if let Some((taken, window)) = self.windowed_lines(s, band) {
            return Ok(taken
                .into_iter()
                .map(|l| {
                    SpectralAtom::new(
                        l.amp,
                        Factors {
                            exp: Some(Exp::at(0.0, TAU * l.hz)),
                            ind: window,
                            ..Factors::NONE
                        },
                        Singular::Regular,
                        s.term.origin,
                    )
                })
                .collect());
        }
        if self.carried(s).is_some() && self.precise(s, band).is_none() {
            return Err(CollapseError::NotEvaluable(
                "a series summed instant by instant",
            ));
        }
        let body = self.expanded(s, band, &mut |id, b| self.summed(id, b))?;
        let held = normalize_read(&body, Var::T, self.reads)
            .map_err(|_| CollapseError::NotEvaluable("a series term"))?;
        let mut atoms = Vec::new();
        for lane in held.lanes {
            let mut summed = Lane::of(lane.atoms);
            for inner in &lane.series {
                summed.atoms.extend(self.atoms(inner, band)?);
            }
            if !lane.series.is_empty() {
                simplify(&mut summed);
            }
            atoms.extend(summed.atoms);
        }
        Ok(atoms)
    }

    /// A line series keeps its terms as runs; anything else is summed term by term.
    fn expanded(
        &self,
        s: &Series,
        band: Audible,
        named: &mut dyn FnMut(NodeId, Audible) -> Result<NodeId, CollapseError>,
    ) -> Result<Body, CollapseError> {
        let taken = self.enumerated(s, band)?;
        if !taken.is_empty() {
            let runs = Run::of(&taken).into_iter();
            return Ok(sum(runs.map(|r| Body::Run(Box::new(r))).collect()));
        }
        if self.precise(s, band).is_none()
            && let Some(banded) = self.banded(s, band, named)?
        {
            return Ok(Body::Banded(Box::new(banded)));
        }
        let count = self.terms(s, band)?;
        let mut parts = Vec::with_capacity(count);
        for i in 0..count {
            let form = self.term(&s.term.body, s.index, (s.lo + i as i64) as f64);
            parts.push(self.written(&form, band, named)?);
        }
        Ok(sum(parts))
    }

    /// One term at index `k`, a ref read at a time the index moves standing for its form read
    /// there.
    fn term(&self, f: &Body, k: IndexId, value: f64) -> Body {
        match f {
            Body::Index(i) if *i == k => Body::Const(C64::real(value)),
            Body::Warp { at, of } if mentions(&at.body, k) => {
                let time = self.term(&at.body, k, value);
                let moved = match &*of.body {
                    Body::Node(id) => self.reads.moved(*id, &time),
                    _ => None,
                };
                match moved {
                    Some(stand) => Body::Node(stand),
                    None => Body::Warp {
                        at: Part::new(at.origin, time),
                        of: Part::new(of.origin, self.term(&of.body, k, value)),
                    },
                }
            }
            other => map_children(other, |p| Part::new(p.origin, self.term(&p.body, k, value))),
        }
    }

    /// A line series whose tail no decay bounds refuses rather than be summed term by term.
    fn enumerated(
        &self,
        s: &Series,
        band: Audible,
    ) -> Result<Vec<sva_formula::Line>, CollapseError> {
        match read_with(&s.term.body, self.reads) {
            Some(Shape::Lines(_)) => {
                let held = (band.ceiling, band.floor_db, band.precision);
                lines_read(s, held, self.reads)
                    .map(|l| l.taken)
                    .ok_or(CollapseError::NotEvaluable(
                        "a series whose tail no bound sums",
                    ))
            }
            _ => Ok(Vec::new()),
        }
    }

    /// A nesting is priced whole before it expands: a written bound counts like the floor's.
    fn terms(&self, s: &Series, band: Audible) -> Result<usize, CollapseError> {
        let cost = self.cost(s, band).ok_or(CollapseError::NotEvaluable(
            "a series whose term count no coefficient bounds",
        ))?;
        if cost.depth == 1 && cost.own > MAX_EXPANDED_TERMS {
            return Err(CollapseError::NotEvaluable(
                "a series of more terms than one instant expands",
            ));
        }
        match cost.terms {
            Some(terms) if terms <= MAX_EXPANDED_TERMS => Ok(cost.own),
            terms => Err(CollapseError::NestedSeries {
                depth: cost.depth,
                terms,
                bound: MAX_EXPANDED_TERMS,
            }),
        }
    }

    fn cost(&self, s: &Series, band: Audible) -> Option<Cost> {
        let own = self.counted(s, band)?;
        let inside = substitute(&s.term.body, s.index, s.lo as f64);
        let (depth, inner) = self.price(&inside, band)?;
        Some(Cost {
            depth: depth + 1,
            terms: inner.and_then(|inner| own.checked_mul(inner)),
            own,
        })
    }

    /// Series counts add under a sum, multiply elsewhere; a part with no series prices nothing.
    fn price(&self, f: &Body, band: Audible) -> Option<Price> {
        match f {
            Body::Series(s) => {
                let cost = self.cost(s, band)?;
                return Some((cost.depth, cost.terms));
            }
            Body::Node(id) => return self.node_price(*id, band),
            _ => {}
        }
        let summed = matches!(f, Body::Add(_));
        let mut depth = 0;
        let mut terms: Option<usize> = Some(if summed { 0 } else { 1 });
        for p in children(f) {
            let (d, n) = self.price(&p.body, band)?;
            if d == 0 {
                continue;
            }
            depth = depth.max(d);
            terms = terms.zip(n).and_then(|(terms, n)| match summed {
                true => terms.checked_add(n),
                false => terms.checked_mul(n),
            });
        }
        Some((depth, terms.map(|n| n.max(1))))
    }

    fn node_price(&self, id: NodeId, band: Audible) -> Option<Price> {
        let key = (id, band.key());
        if let Some(held) = self.prices.borrow().get(&key) {
            return *held;
        }
        let found = match self.reads.view(id, &Read::As) {
            Some(form) => self.price(&form, band),
            None => Some((0, Some(1))),
        };
        self.prices.borrow_mut().insert(key, found);
        found
    }

    /// A geometric magnitude bound stops where its whole tail rounds away; a carrier summed
    /// instant by instant expands at most `MOST_PER_INSTANT` terms an instant.
    fn counted(&self, s: &Series, band: Audible) -> Option<usize> {
        if let Bound::Finite(n) = s.hi {
            return usize::try_from((n - s.lo + 1).max(0)).ok();
        }
        let precise = self.precise(s, band);
        precise.or_else(|| self.carried(s).map(|_| MOST_PER_INSTANT as usize))
    }

    /// The terms before the whole tail of a geometric magnitude bound rounds away.
    fn precise(&self, s: &Series, band: Audible) -> Option<usize> {
        let bound = self.bound(&s.term.body, band)?;
        let ratio = ratio(&bound.body, s.index).filter(|r| *r < 1.0 && bound.exact)?;
        for i in 0..MAX_EXPANDED_TERMS {
            let at = (s.lo + i as i64) as f64;
            let held = exact_constant_at(&bound.body, s.index, at, self.reads)?.abs();
            if held / (1.0 - ratio) <= band.precision {
                return Some(i.max(1));
            }
        }
        None
    }

    /// The angle of a term's one carrier, where it turns at a rate affine in the index and
    /// nothing else in the term moves with both the index and time.
    fn carried<'s>(&self, s: &'s Series) -> Option<&'s Body> {
        if s.hi != Bound::Infinite {
            return None;
        }
        let angle = carrier(&s.term.body, s.index, self.reads)?;
        (degree(angle, s.index)? == 1 && self.real(angle)).then_some(angle)
    }

    /// `s` summed at each instant over only the terms whose carrier turns under the ceiling
    /// there, each turning at the time derivative of its angle read off a spectral sum.
    fn banded(
        &self,
        s: &Series,
        band: Audible,
        named: &mut dyn FnMut(NodeId, Audible) -> Result<NodeId, CollapseError>,
    ) -> Result<Option<Banded>, CollapseError> {
        let Some(angle) = self.carried(s) else {
            return Ok(None);
        };
        let at = |k: f64| substitute(angle, s.index, k);
        let (still, once) = (at(0.0), at(1.0));
        let per = Body::Add(vec![
            Part::bare(once),
            Part::bare(Body::Mul(vec![
                Part::bare(Body::Const(C64::real(-1.0))),
                Part::bare(still.clone()),
            ])),
        ]);
        let turning = |f: &Body| {
            let sum = normalize_read(f, Var::T, self.reads).ok()?;
            let [lane] = sum.lanes.as_slice() else {
                return None;
            };
            let plain = lane.series.is_empty() && lane.modal.is_empty();
            plain.then(|| d_dt(&sum).ok()).flatten()
        };
        let (Some(slope), Some(offset)) = (turning(&per), turning(&still)) else {
            return Ok(None);
        };
        let term = self.written(&s.term.body, band, named)?;
        let weight = steady(&s.term.body, angle);
        let mut banded = Banded {
            series: Series {
                term: Part::new(s.term.origin, term),
                ..s.clone()
            },
            slope,
            offset,
            omega: TAU * band.ceiling,
            most: MOST_PER_INSTANT,
            widest: MOST_PER_INSTANT,
            reach: f64::INFINITY,
            dropped_db: f64::INFINITY,
        };
        self.bounded(&mut banded, &weight, band);
        match banded.dropped_db.is_finite() {
            true => Ok(Some(banded)),
            false => Err(CollapseError::NotEvaluable(
                "a series whose dropped terms no bound holds",
            )),
        }
    }

    /// The most terms an instant sums, a bound on the sum, and the loudest term it may drop.
    fn bounded(&self, b: &mut Banded, weight: &Body, band: Audible) {
        let (lo, k) = (b.series.lo, b.series.index);
        let (fastest, drift) = (sup(&b.slope), sup(&b.offset));
        let (slowest, still) = (least(&b.slope), least(&b.offset).max(0.0));
        let count = |last: f64| (last.ceil().clamp(-9e18, 9e18) as i64 - lo).clamp(0, b.most);
        let kept = match fastest > 0.0 {
            true => count((b.omega - drift) / fastest),
            false if drift < b.omega => b.most,
            false => 0,
        };
        b.widest = match slowest > 0.0 {
            true => count((b.omega + drift.max(still)) / slowest),
            false => b.most,
        };
        let Some(bound) = self.bound(weight, band).filter(|w| w.exact) else {
            return;
        };
        let at = |n: i64| Some(exact_constant_at(&bound.body, k, n as f64, self.reads)?.abs());
        b.reach = (lo..lo + b.widest)
            .try_fold(0.0, |held, n| Some(held + at(n)?))
            .unwrap_or(f64::INFINITY);
        if lo >= 1
            && falls(&bound.body, k)
            && let (Some(top), Some(gone)) = (at(lo), at(lo + kept))
        {
            b.dropped_db = 20.0 * (gone / top).log10();
        }
    }

    fn bound(&self, f: &Body, band: Audible) -> Option<Bounded> {
        let held = |body: Body, exact: bool| Some(Bounded { body, exact });
        let under = |p: &Part| {
            self.bound(&p.body, band)
                .map(|b| (Part::new(p.origin, b.body), b.exact))
        };
        let all = |parts: &[Part], wrap: &dyn Fn(Part) -> Part| {
            let mut exact = true;
            let mut kept = Vec::with_capacity(parts.len());
            for p in parts {
                let (part, known) = under(p)?;
                exact &= known;
                kept.push(wrap(part));
            }
            Some((kept, exact))
        };
        let mentions_line = |f: &Body| mentions_line_read(f, self.reads);
        match f {
            Body::Series(s) => match self.total(s, band) {
                Some(sum) => held(Body::Const(C64::real(sum)), true),
                None => held(Body::Const(C64::ONE), false),
            },
            Body::Node(id) => self.node_bound(*id, band),
            Body::Line => held(Body::Const(C64::ONE), false),
            Body::Mul(parts) => {
                let (kept, exact) = all(parts, &|p| p)?;
                held(Body::Mul(kept), exact)
            }
            Body::Add(parts) => {
                let (kept, exact) = all(parts, &|p| Part::bare(Body::Apply(Unary::Abs, p)))?;
                held(Body::Add(kept), exact)
            }
            Body::Div(a, b) if !mentions_line(&b.body) => {
                let (num, exact) = under(a)?;
                held(Body::Div(num, b.clone()), exact)
            }
            Body::Div(a, b) => {
                let ((num, _), (den, _)) = (under(a)?, under(b)?);
                held(Body::Div(num, den), false)
            }
            Body::Apply(Unary::Sin | Unary::Cos | Unary::Tanh | Unary::Sat | Unary::Step, arg) => {
                held(Body::Const(C64::ONE), self.real(&arg.body))
            }
            Body::Crop { of, .. } | Body::Shift { of, .. } | Body::Warp { of, .. } => {
                self.bound(&of.body, band)
            }
            Body::Pow(base, n) => {
                let (part, exact) = under(base)?;
                held(Body::Pow(part, *n), exact && *n >= 0)
            }
            other if !mentions_line(other) => held(other.clone(), true),
            _ => None,
        }
    }

    /// A ref holds no index, so its bound is one number; one no number holds bounds nothing.
    fn node_bound(&self, id: NodeId, band: Audible) -> Option<Bounded> {
        let key = (id, band.key());
        if let Some(held) = self.bounds.borrow().get(&key) {
            return held.clone();
        }
        let found = match self.reads.view(id, &Read::As) {
            Some(form) => self.bound(&form, band).and_then(|b| {
                let value = exact_constant_read(&b.body, self.reads)?;
                Some(Bounded {
                    body: Body::Const(value),
                    exact: b.exact,
                })
            }),
            None => Some(Bounded {
                body: Body::Const(C64::ONE),
                exact: false,
            }),
        };
        self.bounds.borrow_mut().insert(key, found.clone());
        found
    }

    /// A geometric series sums to its first bound over `1 - ratio`; any other to what it keeps.
    fn total(&self, s: &Series, band: Audible) -> Option<f64> {
        let bound = self.bound(&s.term.body, band).filter(|b| b.exact)?.body;
        let at = |i: i64| Some(exact_constant_at(&bound, s.index, i as f64, self.reads)?.abs());
        if let (Bound::Infinite, Some(r)) = (s.hi, ratio(&bound, s.index).filter(|r| *r < 1.0)) {
            return Some(at(s.lo)? / (1.0 - r));
        }
        let kept =
            i64::try_from(self.counted(s, band).filter(|n| *n <= MAX_EXPANDED_TERMS)?).ok()?;
        (s.lo..s.lo + kept).try_fold(0.0, |held, i| Some(held + at(i)?))
    }

    fn real(&self, f: &Body) -> bool {
        axis_read(f, &Unread, self.reads) == Axis::Real
    }
}

fn sum(parts: Vec<Body>) -> Body {
    match parts.len() {
        0 => Body::Const(C64::ZERO),
        1 => parts.into_iter().next().expect("one part"),
        _ => Body::Add(parts.into_iter().map(Part::bare).collect()),
    }
}

/// The one `sin` or `cos` whose argument moves with the index, where nothing else moving with
/// both the index and time is in the term.
fn carrier<'f>(f: &'f Body, k: IndexId, reads: &dyn Reads) -> Option<&'f Body> {
    let (mut found, mut clean) = (Vec::new(), true);
    moving(f, k, reads, (&mut found, &mut clean));
    match found.as_slice() {
        [angle] if clean => Some(*angle),
        _ => None,
    }
}

fn moving<'f>(
    f: &'f Body,
    k: IndexId,
    reads: &dyn Reads,
    (found, clean): (&mut Vec<&'f Body>, &mut bool),
) {
    if !mentions(f, k) {
        return;
    }
    match f {
        Body::Apply(Unary::Sin | Unary::Cos, arg) => found.push(&arg.body),
        Body::Apply(_, arg) if mentions_line_read(&arg.body, reads) => *clean = false,
        Body::Warp { .. }
        | Body::Series(_)
        | Body::Banded(_)
        | Body::Keyed { .. }
        | Body::Delta { .. }
        | Body::Pv(_)
        | Body::Deriv { .. } => *clean = false,
        other => {
            for p in children(other) {
                moving(&p.body, k, reads, (&mut *found, &mut *clean));
            }
        }
    }
}

/// How many times the index multiplies into `f`, where it is a polynomial in it.
fn degree(f: &Body, k: IndexId) -> Option<u32> {
    if !mentions(f, k) {
        return Some(0);
    }
    match f {
        Body::Index(i) if *i == k => Some(1),
        Body::Add(parts) => parts
            .iter()
            .try_fold(0, |held, p| Some(held.max(degree(&p.body, k)?))),
        Body::Mul(parts) => parts
            .iter()
            .try_fold(0, |held, p| Some(held + degree(&p.body, k)?)),
        Body::Div(num, den) if !mentions(&den.body, k) => degree(&num.body, k),
        Body::Pow(base, n) if *n >= 0 => Some(degree(&base.body, k)? * n.unsigned_abs()),
        _ => None,
    }
}

/// `term` with its carrier at its loudest: the weight bounding every term's magnitude.
fn steady(term: &Body, angle: &Body) -> Body {
    match term {
        Body::Apply(Unary::Sin | Unary::Cos, arg) if std::ptr::eq(&*arg.body, angle) => {
            Body::Const(C64::ONE)
        }
        other => map_children(other, |p| Part::new(p.origin, steady(&p.body, angle))),
    }
}

/// At least `sup |f(t)|` over every instant; infinite where an atom grows without bound.
fn sup(f: &SpectralSum) -> f64 {
    let atoms = f.lanes.iter().flat_map(|lane| lane.atoms.iter());
    atoms
        .map(|a| sup_from(a, f64::NEG_INFINITY).unwrap_or(f64::INFINITY))
        .sum()
}

/// At most `inf |f(t)|` over every instant: its constant less all that moves it.
fn least(f: &SpectralSum) -> f64 {
    let atoms = f.lanes.iter().flat_map(|lane| lane.atoms.iter());
    let (still, moving): (Vec<_>, Vec<_>) = atoms.partition(|a| a.is_bare());
    let held = still
        .iter()
        .fold(C64::ZERO, |held, a: &&SpectralAtom| held + a.c)
        .abs();
    let moves: f64 = moving
        .iter()
        .map(|a| sup_from(a, f64::NEG_INFINITY).unwrap_or(f64::INFINITY))
        .sum();
    (held - moves).max(0.0)
}

struct Unread;

impl Env for Unread {
    fn node(&self, _: NodeId) -> Ty {
        Ty::form(Var::T, false, Codomain::Complex)
    }

    fn param(&self, _: ParamId) -> Ty {
        Ty::form(Var::T, false, Codomain::Complex)
    }
}
