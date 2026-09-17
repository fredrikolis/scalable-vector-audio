// Concern: content-addresses a SpectralSum under the table version, and keys the random constant | Non-concern: what a hash keys (sva-engine) | IO: (&SpectralSum) -> Hash

use std::fmt;

use crate::closed_form::{
    Body, Bound, ClosedForm, Edge, Excitation, Fold, ModalBank, Mode, Part, Rational, Series,
    Unary, Var,
};
use crate::complex::{C64, canonical};
use crate::lanes::Lanes;
use crate::spectral_sum::atom::{Singular, SpectralAtom};
use crate::spectral_sum::{Lane, SpectralSum};
use crate::table::TABLE_VERSION;

/// Two independently primed FNV-1a lanes: serving one closed form's samples for another is silent
/// corruption, so the width is set against birthday collisions rather than speed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Hash(pub u64, pub u64);

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}{:016x}", self.0, self.1)
    }
}

pub fn hash_spectral_sum(n: &SpectralSum) -> Hash {
    hash_spectral_sum_under(n, TABLE_VERSION)
}

pub fn hash_closed_form(t: &ClosedForm) -> Hash {
    hash_closed_form_under(t, TABLE_VERSION)
}

/// The version is the first field after the tag, so a table bump retires every entry keyed
/// by one of these.
pub fn hash_spectral_sum_under(n: &SpectralSum, table_version: u64) -> Hash {
    let mut s = Sink::new(0x01, table_version);
    s.var(n.var);
    s.u64(n.lanes.len() as u64);
    for lane in &n.lanes {
        s.lane(lane);
    }
    s.finish()
}

pub fn hash_closed_form_under(t: &ClosedForm, table_version: u64) -> Hash {
    let mut s = Sink::new(0x02, table_version);
    s.var(t.var);
    s.formula(&t.body);
    s.finish()
}

/// The unit-interval value one key and one seed name, wherever the pair is written.
pub fn draw(seed: u64, key: f64) -> f64 {
    keyed(&format!("{key}"), seed) as f64 / u64::MAX as f64
}

/// The keyed constant `rand(key, seed=)` reads, rate-free and render-free.
pub fn keyed(key: &str, seed: u64) -> u64 {
    let mut s = Sink::new(0x03, TABLE_VERSION);
    s.bytes(key.as_bytes());
    s.u64(seed);
    s.finish().0
}

struct Sink(Lanes<0>);

impl Sink {
    fn new(tag: u8, table_version: u64) -> Sink {
        let mut s = Sink(Lanes::default());
        s.byte(tag);
        s.u64(table_version);
        s
    }

    fn byte(&mut self, b: u8) {
        self.0.word(u64::from(b));
    }

    /// Length-prefixed, so `("ab", "c")` and `("a", "bc")` cannot encode alike.
    fn bytes(&mut self, b: &[u8]) {
        self.u64(b.len() as u64);
        for &x in b {
            self.byte(x);
        }
    }

    fn u64(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.byte(b);
        }
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

    fn var(&mut self, v: Var) {
        self.byte(match v {
            Var::T => 0,
            Var::F => 1,
        });
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
        self.u64(u64::from(a.poly));
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
        self.u64(u64::from(s.index.0));
        self.i64(s.lo);
        match s.hi {
            Bound::Finite(n) => {
                self.byte(0);
                self.i64(n);
            }
            Bound::Infinite => self.byte(1),
        }
        self.formula(&s.term.body);
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
            self.formula(&p.body);
        }
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
                self.byte(0x10);
                self.c64(*c);
            }
            Body::Line => self.byte(0x11),
            Body::Index(i) => {
                self.byte(0x12);
                self.u64(u64::from(i.0));
            }
            Body::Param(p) => {
                self.byte(0x13);
                self.u64(u64::from(p.0));
            }
            Body::Node(n) => {
                self.byte(0x14);
                self.u64(u64::from(n.0));
            }
            Body::Add(parts) => {
                self.byte(0x15);
                self.parts(parts);
            }
            Body::Mul(parts) => {
                self.byte(0x16);
                self.parts(parts);
            }
            Body::Div(a, b) => {
                self.byte(0x17);
                self.formula(&a.body);
                self.formula(&b.body);
            }
            Body::Pow(base, n) => {
                self.byte(0x18);
                self.formula(&base.body);
                self.i64(i64::from(*n));
            }
            Body::Apply(op, arg) => {
                self.byte(0x19);
                self.byte(unary_tag(*op));
                self.formula(&arg.body);
            }
            Body::Fold(op, args) => {
                self.byte(0x1a);
                self.byte(match op {
                    Fold::Max => 0,
                    Fold::Min => 1,
                    Fold::Mod => 2,
                });
                self.parts(args);
            }
            Body::Delta { at, order } => {
                self.byte(0x1b);
                self.formula(&at.body);
                self.u64(u64::from(*order));
            }
            Body::Pv(at) => {
                self.byte(0x1c);
                self.formula(&at.body);
            }
            Body::Warp { at, of } => {
                self.byte(0x27);
                self.formula(&at.body);
                self.formula(&of.body);
            }
            Body::Shift { by, of } => {
                self.byte(0x1d);
                self.f64(*by);
                self.formula(&of.body);
            }
            Body::Deriv { order, of } => {
                self.byte(0x1e);
                self.u64(u64::from(*order));
                self.formula(&of.body);
            }
            Body::Crop {
                of,
                l,
                r,
                rise,
                fall,
            } => {
                self.byte(0x1f);
                self.formula(&of.body);
                self.edge(*l);
                self.edge(*r);
                self.f64(*rise);
                self.f64(*fall);
            }
            Body::Join(parts) => {
                self.byte(0x21);
                self.parts(parts);
            }
            Body::Channel(of, k) => {
                self.byte(0x22);
                self.formula(&of.body);
                self.byte(*k);
            }
            Body::Rational(r) => {
                self.byte(0x23);
                self.rational(r);
            }
            Body::Series(s) => {
                self.byte(0x24);
                self.series(s);
            }
            Body::Modal(m) => {
                self.byte(0x25);
                self.modal(m);
            }
            Body::Keyed { seed, of } => {
                self.byte(0x26);
                self.u64(*seed);
                self.formula(&of.body);
            }
        }
    }

    /// One avalanche past the lanes, so a short input still fills both words.
    fn finish(&self) -> Hash {
        let Hash(a, b) = self.0.finish();
        Hash(mix(a), mix(b))
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
    }
}

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
