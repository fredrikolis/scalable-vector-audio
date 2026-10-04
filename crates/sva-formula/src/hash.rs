// Concern: content-addresses a written closed form and a read's time, and draws a seed at a step | Non-concern: what a hash keys (sva-engine) | IO: (&ClosedForm, a namer) -> Hash

use std::fmt;

use crate::closed_form::{
    Body, Bound, ClosedForm, Edge, Excitation, Fold, IndexId, ModalBank, Mode, Part, Rational,
    Series, Unary, Var,
};
use crate::complex::{C64, canonical};
use crate::content_hash::{ContentHasher, HashDomain};
use crate::env::NodeId;
use crate::fourier_dual::FOURIER_DUAL_RULES_VERSION;
use crate::run::Mirror;
use crate::spectral_sum::Lane;
use crate::spectral_sum::atom::{Singular, SpectralAtom};

/// Serving one closed form's samples for another is silent corruption, so the width is set
/// against birthday collisions rather than speed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Hash(pub u64, pub u64);

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}{:016x}", self.0, self.1)
    }
}

/// Two operands of a sum or product, which IEEE commutes bit for bit; three never associate so.
pub fn either_order(operands: &mut [Hash]) {
    if let [a, b] = operands
        && b < a
    {
        std::mem::swap(a, b);
    }
}

/// A closed form as written, each subterm by its own hash and each ref as `node` names it, so a
/// node named by its form's hash reads alike by ref or written in place.
pub fn hash_written_with(t: &ClosedForm, node: &mut dyn FnMut(NodeId) -> Hash) -> Hash {
    let mut s = Sink {
        hasher: ContentHasher::new(HashDomain::WrittenClosedForm),
        node: Some(node),
        bound: Vec::new(),
        free: true,
        merkle: true,
    };
    let held = s.part(&t.body);
    match t.var {
        Var::T => held,
        Var::F => {
            let mut f = ContentHasher::new(HashDomain::ClosedFormInFrequency);
            f.hash(held);
            f.finish()
        }
    }
}

/// A time a ref is read at, as a reading of that ref is kept under: an index it holds is one
/// series' own, named by its number.
pub fn hash_time(at: &Body) -> Hash {
    let mut s = Sink {
        hasher: ContentHasher::new(HashDomain::ReadTime),
        node: None,
        bound: Vec::new(),
        free: true,
        merkle: false,
    };
    s.u64(FOURIER_DUAL_RULES_VERSION);
    s.formula(at);
    s.finish()
}

/// The unit-interval value one seed draws at one whole step, wherever the pair is written.
pub fn draw(seed: u64, step: i64) -> f64 {
    mix(mix(seed ^ DRAWN) ^ step as u64) as f64 / u64::MAX as f64
}

const DRAWN: u64 = 0x9e37_79b9_7f4a_7c15;

/// The draw at the whole step nearest `key`, ties to even; `None` past any `i64` step.
pub fn draw_nearest(seed: u64, key: f64) -> Option<f64> {
    let step = key.round_ties_even();
    let held = step >= i64::MIN as f64 && step < i64::MAX as f64;
    held.then(|| draw(seed, step as i64))
}

struct Sink<'a> {
    hasher: ContentHasher,
    node: Option<&'a mut dyn FnMut(NodeId) -> Hash>,
    /// The series indices bound around the term being hashed, innermost last: an index is
    /// hashed by which binder it names, never by the number a typing drew for it.
    bound: Vec<IndexId>,
    /// Whether an index no binder here names is named by its own number.
    free: bool,
    /// Each subterm named by its own hash, as a ref is by its node's.
    merkle: bool,
}

impl<'a> Sink<'a> {
    fn byte(&mut self, b: u8) {
        self.hasher.word(u64::from(b));
    }

    fn u64(&mut self, v: u64) {
        self.hasher.word(v);
    }

    fn i64(&mut self, v: i64) {
        self.u64(v as u64);
    }

    fn f64(&mut self, v: f64) {
        self.u64(canonical(v));
    }

    fn c64(&mut self, v: C64) {
        let (re, im) = v.bits();
        self.u64(re);
        self.u64(im);
    }

    fn edge(&mut self, e: Edge) {
        match e {
            Edge::NegInf => self.byte(0),
            Edge::At(bits) => {
                self.byte(1);
                self.u64(bits);
            }
            Edge::PosInf => self.byte(2),
        }
    }

    fn lane(&mut self, lane: &Lane) {
        self.u64(lane.atoms.len() as u64);
        for a in &lane.atoms {
            self.atom(a);
        }
        self.u64(lane.series.len() as u64);
        for s in &lane.series {
            self.series(s);
        }
        self.u64(lane.modal.len() as u64);
        for m in &lane.modal {
            self.modal(m);
        }
    }

    /// `Origin` is excluded: two spellings from different files must hash alike.
    fn atom(&mut self, a: &SpectralAtom) {
        self.c64(a.c);
        self.u64(u64::from(a.poly.degree));
        self.f64(a.poly.at);
        match a.exp {
            None => self.byte(0),
            Some(e) => {
                self.byte(1);
                self.f64(e.sigma);
                self.f64(e.omega);
            }
        }
        match a.gauss {
            None => self.byte(0),
            Some(g) => {
                self.byte(1);
                self.f64(g.a);
                self.f64(g.mu);
            }
        }
        match a.ind {
            None => self.byte(0),
            Some(i) => {
                self.byte(1);
                self.edge(i.l);
                self.edge(i.r);
            }
        }
        match a.pole {
            None => self.byte(0),
            Some(p) => {
                self.byte(1);
                self.c64(p.at);
                self.u64(u64::from(p.order));
                self.byte(u8::from(p.pv));
            }
        }
        match a.sing {
            Singular::Regular => self.byte(0),
            Singular::Delta { at, order } => {
                self.byte(1);
                self.f64(at);
                self.u64(u64::from(order));
            }
        }
    }

    fn series(&mut self, s: &Series) {
        self.i64(s.lo);
        match s.hi {
            Bound::Finite(n) => {
                self.byte(0);
                self.i64(n);
            }
            Bound::Infinite => self.byte(1),
        }
        self.bound.push(s.index);
        self.formula(&s.term.body);
        self.bound.pop();
    }

    fn amps(&mut self, amps: &[C64]) {
        self.u64(amps.len() as u64);
        for a in amps {
            self.c64(*a);
        }
    }

    fn modal(&mut self, m: &ModalBank) {
        self.u64(m.modes.len() as u64);
        for Mode {
            omega,
            tau,
            amp,
            phase,
        } in &m.modes
        {
            self.f64(*omega);
            self.f64(*tau);
            self.f64(*amp);
            self.f64(*phase);
        }
        match m.excite {
            Excitation::HammerPulse { f0, t0, contact } => {
                self.byte(0);
                self.f64(f0);
                self.f64(t0);
                self.f64(contact);
            }
            Excitation::Impulse { t0 } => {
                self.byte(1);
                self.f64(t0);
            }
        }
    }

    fn parts(&mut self, parts: &[Part]) {
        self.u64(parts.len() as u64);
        for p in parts {
            self.child(&p.body);
        }
    }

    fn commuting(&mut self, parts: &[Part]) {
        let [a, b] = parts else {
            return self.parts(parts);
        };
        if !self.merkle {
            return self.parts(parts);
        }
        let mut held = [self.part(&a.body), self.part(&b.body)];
        either_order(&mut held);
        self.u64(2);
        for Hash(x, y) in held {
            self.u64(x);
            self.u64(y);
        }
    }

    fn child(&mut self, f: &Body) {
        match self.merkle {
            true => {
                let Hash(x, y) = self.part(f);
                self.u64(x);
                self.u64(y);
            }
            false => self.formula(f),
        }
    }

    fn part(&mut self, f: &Body) -> Hash {
        if let Body::Node(n) = f
            && let Some(named) = self.node.as_mut()
        {
            return named(*n);
        }
        let mut sub = Sink {
            hasher: ContentHasher::new(HashDomain::WrittenClosedForm),
            node: self.node.take(),
            bound: std::mem::take(&mut self.bound),
            free: self.free,
            merkle: true,
        };
        sub.formula(f);
        self.node = sub.node.take();
        self.bound = std::mem::take(&mut sub.bound);
        sub.finish()
    }

    fn rational(&mut self, r: &Rational) {
        self.u64(r.zeros.len() as u64);
        for z in &r.zeros {
            self.c64(*z);
        }
        self.u64(r.poles.len() as u64);
        for p in &r.poles {
            self.c64(*p);
        }
        self.c64(r.gain);
    }

    fn formula(&mut self, f: &Body) {
        match f {
            Body::Const(c) => {
                self.byte(tag::CONST);
                self.c64(*c);
            }
            Body::Line => self.byte(tag::LINE),
            Body::Index(i) => match self.bound.iter().rev().position(|b| b == i) {
                Some(depth) => {
                    self.byte(tag::BOUND_INDEX);
                    self.u64(depth as u64);
                }
                None => {
                    assert!(self.free, "an index inside the series binding it");
                    self.byte(tag::FREE_INDEX);
                    self.u64(u64::from(i.0));
                }
            },
            Body::Param(p) => {
                self.byte(tag::PARAM);
                self.u64(u64::from(p.0));
            }
            Body::Node(n) => {
                self.byte(tag::NODE);
                match self.node.as_mut().map(|named| named(*n)) {
                    Some(held) => {
                        self.u64(held.0);
                        self.u64(held.1);
                    }
                    None => self.u64(u64::from(n.0)),
                }
            }
            Body::Add(parts) => {
                self.byte(tag::ADD);
                self.commuting(parts);
            }
            Body::Mul(parts) => {
                self.byte(tag::MUL);
                self.commuting(parts);
            }
            Body::Div(a, b) => {
                self.byte(tag::DIV);
                self.child(&a.body);
                self.child(&b.body);
            }
            Body::Pow(base, n) => {
                self.byte(tag::POW);
                self.child(&base.body);
                self.i64(i64::from(*n));
            }
            Body::Apply(op, arg) => {
                self.byte(tag::APPLY);
                self.byte(unary_tag(*op));
                self.child(&arg.body);
            }
            Body::Fold(op, args) => {
                self.byte(tag::FOLD);
                self.byte(match op {
                    Fold::Max => 0,
                    Fold::Min => 1,
                    Fold::Mod => 2,
                });
                self.parts(args);
            }
            Body::Delta { at, order } => {
                self.byte(tag::DELTA);
                self.child(&at.body);
                self.u64(u64::from(*order));
            }
            Body::Pv(at) => {
                self.byte(tag::PV);
                self.child(&at.body);
            }
            Body::Warp { at, of } => {
                self.byte(tag::WARP);
                self.child(&at.body);
                self.child(&of.body);
            }
            Body::Shift { by, of } => {
                self.byte(tag::SHIFT);
                self.f64(*by);
                self.child(&of.body);
            }
            Body::Deriv { order, of } => {
                self.byte(tag::DERIV);
                self.u64(u64::from(*order));
                self.child(&of.body);
            }
            Body::Crop {
                of,
                l,
                r,
                rise,
                fall,
            } => {
                self.byte(tag::CROP);
                self.child(&of.body);
                self.edge(*l);
                self.edge(*r);
                self.f64(*rise);
                self.f64(*fall);
            }
            Body::Join(parts) => {
                self.byte(tag::JOIN);
                self.parts(parts);
            }
            Body::Channel(of, k) => {
                self.byte(tag::CHANNEL);
                self.child(&of.body);
                self.byte(*k);
            }
            Body::Rational(r) => {
                self.byte(tag::RATIONAL);
                self.rational(r);
            }
            Body::Series(s) => {
                self.byte(tag::SERIES);
                self.series(s);
            }
            Body::Modal(m) => {
                self.byte(tag::MODAL);
                self.modal(m);
            }
            Body::Run(run) => {
                self.byte(tag::RUN);
                self.f64(run.offset);
                self.f64(run.step);
                self.i64(run.first);
                self.amps(&run.amps);
                match &run.mirror {
                    Mirror::None => self.byte(0),
                    Mirror::Conjugate => self.byte(1),
                    Mirror::Held(amps) => {
                        self.byte(2);
                        self.amps(amps);
                    }
                }
            }
            Body::Banded(b) => {
                self.byte(tag::BANDED);
                self.series(&b.series);
                for sum in [&b.slope, &b.offset] {
                    self.u64(sum.lanes.len() as u64);
                    sum.lanes.iter().for_each(|lane| self.lane(lane));
                }
                for x in [b.omega, b.reach, b.dropped_db] {
                    self.f64(x);
                }
                self.i64(b.most);
                self.i64(b.widest);
            }
            Body::Keyed { seed, of } => {
                self.byte(tag::KEYED);
                self.u64(*seed);
                self.child(&of.body);
            }
        }
    }

    fn finish(self) -> Hash {
        self.hasher.finish()
    }
}

fn unary_tag(op: Unary) -> u8 {
    match op {
        Unary::Sin => 0,
        Unary::Cos => 1,
        Unary::Exp => 2,
        Unary::Tanh => 3,
        Unary::Sat => 4,
        Unary::Abs => 5,
        Unary::Log => 6,
        Unary::Sqrt => 7,
        Unary::Step => 8,
    }
}

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The byte each body opens with: one per meaning, so no two bodies share a prefix.
mod tag {
    pub const CONST: u8 = 0x10;
    pub const LINE: u8 = 0x11;
    pub const BOUND_INDEX: u8 = 0x12;
    pub const PARAM: u8 = 0x13;
    pub const NODE: u8 = 0x14;
    pub const ADD: u8 = 0x15;
    pub const MUL: u8 = 0x16;
    pub const DIV: u8 = 0x17;
    pub const POW: u8 = 0x18;
    pub const APPLY: u8 = 0x19;
    pub const FOLD: u8 = 0x1a;
    pub const DELTA: u8 = 0x1b;
    pub const PV: u8 = 0x1c;
    pub const SHIFT: u8 = 0x1d;
    pub const DERIV: u8 = 0x1e;
    pub const CROP: u8 = 0x1f;
    pub const JOIN: u8 = 0x21;
    pub const CHANNEL: u8 = 0x22;
    pub const RATIONAL: u8 = 0x23;
    pub const SERIES: u8 = 0x24;
    pub const MODAL: u8 = 0x25;
    pub const KEYED: u8 = 0x26;
    pub const WARP: u8 = 0x27;
    pub const RUN: u8 = 0x28;
    pub const FREE_INDEX: u8 = 0x29;
    pub const BANDED: u8 = 0x2a;

    #[cfg(test)]
    pub const ALL: [u8; 26] = [
        CONST,
        LINE,
        BOUND_INDEX,
        PARAM,
        NODE,
        ADD,
        MUL,
        DIV,
        POW,
        APPLY,
        FOLD,
        DELTA,
        PV,
        SHIFT,
        DERIV,
        CROP,
        JOIN,
        CHANNEL,
        RATIONAL,
        SERIES,
        MODAL,
        KEYED,
        WARP,
        RUN,
        FREE_INDEX,
        BANDED,
    ];
}

#[cfg(test)]
mod tests {
    use super::tag;

    /// A tag two bodies share makes the encoding ambiguous: a free index and a banded series
    /// once both opened with 0x29.
    #[test]
    fn every_body_has_its_own_tag() {
        let mut tags = tag::ALL.to_vec();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), tag::ALL.len());
    }
}
