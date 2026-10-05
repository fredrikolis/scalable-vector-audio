// Concern: evaluates a written form over a column of instants, each identical subterm once | Non-concern: which row takes it, one atom's value | IO: (&Body, refs, instants) -> each lane's value

use std::collections::HashMap;

use sva_formula::closed_form::{Fold, Unary};
use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::{Banded, Body, C64, IndexId, Run};

use super::point::{crop_gain, eval_atom_on, eval_lane_on, lane_of, shoulders, unary};
use crate::error::CollapseError;
use crate::grid::Grid;

/// One component of a written form as ops, operands before the ops reading them: every
/// subterm the form, or a form it reads, writes alike in the same time is one op.
#[derive(Clone, Debug)]
pub(crate) struct Program {
    ops: Vec<Op>,
    roots: Vec<usize>,
}

/// Slot 0 is the instant each lane is evaluated at; a time slot holds an instant in `re`.
#[derive(Clone, Debug)]
enum Op {
    Time,
    Const(C64),
    Fail(CollapseError),
    Line(usize),
    Shifted(usize, f64),
    /// A warp's instant: its time's real part, or 0 and the time's refusal, which `Warp` reads.
    Re(usize),
    Warp {
        when: usize,
        of: usize,
    },
    Add(Vec<usize>),
    Mul(Vec<usize>),
    Div(usize, usize),
    Pow(usize, i32),
    Apply(Unary, usize),
    Fold(Fold, Vec<usize>),
    Crop {
        time: usize,
        on: bool,
        of: usize,
        l: f64,
        r: f64,
        rise: f64,
        fall: f64,
    },
    Singular {
        time: usize,
        order: i32,
    },
    Keyed {
        seed: u64,
        of: usize,
    },
    Modal {
        time: usize,
        on: bool,
        atoms: Vec<SpectralAtom>,
    },
    Run {
        time: usize,
        run: Box<Run>,
    },
    /// The value bound to the `k`-th index a series around this program sums over.
    Bound(usize),
    Banded {
        time: usize,
        on: bool,
        banded: Box<Banded>,
        term: Box<Program>,
    },
}

pub(super) const INFINITE: CollapseError =
    CollapseError::NotEvaluable("a division or a remainder by zero, or an infinite value");

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Cx {
    time: usize,
    on: bool,
    component: usize,
}

struct Lowering<'a> {
    refs: &'a [Body],
    /// Each ref's component count, folded once per ref.
    widths: Vec<usize>,
    bound: Vec<IndexId>,
    ops: Vec<Op>,
    held: HashMap<Vec<u64>, usize>,
    /// Each ref already lowered in a time, by its id.
    read: HashMap<(u32, Cx), usize>,
}

impl Program {
    /// Each of `roots` at `component`, `Body::Node(k)` in any of them, or in a ref, naming
    /// `refs[k]`; a node past `refs` refuses where it is evaluated.
    pub(crate) fn of(roots: &[&Body], refs: &[Body], component: usize) -> Program {
        Program::bound(roots, refs, (component, true), Vec::new())
    }

    fn bound(
        roots: &[&Body],
        refs: &[Body],
        (component, on): (usize, bool),
        bound: Vec<IndexId>,
    ) -> Program {
        let mut widths = Vec::with_capacity(refs.len());
        for body in refs {
            widths.push(width_of(body, &widths));
        }
        let mut lowering = Lowering {
            refs,
            widths,
            bound,
            ops: vec![Op::Time],
            held: HashMap::new(),
            read: HashMap::new(),
        };
        let cx = Cx {
            time: 0,
            on,
            component,
        };
        let roots = roots.iter().map(|body| lowering.lower(body, cx)).collect();
        Program {
            ops: lowering.ops,
            roots,
        }
    }

    /// The program's values at `lanes` instants; `Columns::root` evaluates on demand.
    pub(crate) fn columns(&self, lanes: usize) -> Columns<'_> {
        Columns {
            program: self,
            t: vec![0.0; lanes],
            on: vec![None; lanes],
            grid: Grid::of(1),
            bound: Vec::new(),
            cols: vec![Col::default(); self.ops.len()],
            done: vec![(0, 0); self.ops.len()],
            lanes,
        }
    }
}

impl Lowering<'_> {
    fn push(&mut self, op: Op) -> usize {
        let op = self.folded(op);
        let key = key(&op);
        if let Some(slot) = key.as_ref().and_then(|k| self.held.get(k)) {
            return *slot;
        }
        self.ops.push(op);
        let slot = self.ops.len() - 1;
        if let Some(key) = key {
            self.held.insert(key, slot);
        }
        slot
    }

    /// An op reading only constants is the value it takes at every instant, or its refusal.
    fn folded(&self, op: Op) -> Op {
        let args = operands(&op);
        if args.is_empty() || !args.iter().all(|a| matches!(self.ops[*a], Op::Const(_))) {
            return op;
        }
        let mut ops = vec![Op::Time];
        ops.extend(args.iter().map(|a| self.ops[*a].clone()));
        ops.push(with_operands(
            op.clone(),
            &(1..=args.len()).collect::<Vec<_>>(),
        ));
        let alone = Program {
            roots: vec![ops.len() - 1],
            ops,
        };
        let mut columns = alone.columns(1);
        match columns.root(0, (0, 1)).get(0) {
            Ok(v) => Op::Const(v),
            Err(e) => Op::Fail(e.clone()),
        }
    }

    fn fail(&mut self, e: CollapseError) -> usize {
        self.push(Op::Fail(e))
    }

    fn each(&mut self, parts: &[sva_formula::Part], cx: Cx) -> Vec<usize> {
        parts.iter().map(|p| self.lower(&p.body, cx)).collect()
    }

    fn lower(&mut self, f: &Body, cx: Cx) -> usize {
        match f {
            Body::Const(c) if c.is_finite() => self.push(Op::Const(*c)),
            Body::Const(_) => self.fail(INFINITE),
            Body::Line => self.push(Op::Line(cx.time)),
            Body::Add(parts) => {
                let parts = self.each(parts, cx);
                self.push(Op::Add(parts))
            }
            Body::Mul(parts) => {
                let parts = self.each(parts, cx);
                self.push(Op::Mul(parts))
            }
            Body::Div(a, b) => {
                let (a, b) = (self.lower(&a.body, cx), self.lower(&b.body, cx));
                self.push(Op::Div(a, b))
            }
            Body::Pow(a, n) => {
                let a = self.lower(&a.body, cx);
                self.push(Op::Pow(a, *n))
            }
            Body::Apply(op, a) => {
                let a = self.lower(&a.body, cx);
                self.push(Op::Apply(*op, a))
            }
            Body::Fold(op, parts) => {
                assert!(!parts.is_empty(), "a fold holds one part");
                let parts = self.each(parts, cx);
                self.push(Op::Fold(*op, parts))
            }
            Body::Shift { by, of } => {
                let time = self.push(Op::Shifted(cx.time, *by));
                self.lower(
                    &of.body,
                    Cx {
                        time,
                        on: false,
                        ..cx
                    },
                )
            }
            Body::Warp { at, of } => {
                let when = self.lower(&at.body, cx);
                let time = self.push(Op::Re(when));
                let of = self.lower(
                    &of.body,
                    Cx {
                        time,
                        on: false,
                        ..cx
                    },
                );
                self.push(Op::Warp { when: time, of })
            }
            Body::Crop {
                of,
                l,
                r,
                rise,
                fall,
            } => {
                let of = self.lower(&of.body, cx);
                self.push(Op::Crop {
                    time: cx.time,
                    on: cx.on,
                    of,
                    l: l.value(),
                    r: r.value(),
                    rise: *rise,
                    fall: *fall,
                })
            }
            Body::Channel(of, k) => self.lower(
                &of.body,
                Cx {
                    component: usize::from(*k),
                    ..cx
                },
            ),
            Body::Delta { order, .. } => self.push(Op::Singular {
                time: cx.time,
                order: i32::from(*order),
            }),
            Body::Pv(_) => self.push(Op::Singular {
                time: cx.time,
                order: -1,
            }),
            Body::Keyed { seed, of } => {
                let of = self.lower(&of.body, cx);
                self.push(Op::Keyed { seed: *seed, of })
            }
            Body::Join(parts) => {
                let widths: Vec<usize> = parts
                    .iter()
                    .map(|p| width_of(&p.body, &self.widths))
                    .collect();
                match lane_of(&widths, cx.component) {
                    Some((lane, component)) => {
                        self.lower(&parts[lane].body, Cx { component, ..cx })
                    }
                    None => self.fail(CollapseError::NotEvaluable("a component past the width")),
                }
            }
            Body::Modal(bank) => self.push(Op::Modal {
                time: cx.time,
                on: cx.on,
                atoms: sva_formula::modal::atoms(bank, sva_formula::Origin::UNKNOWN),
            }),
            Body::Node(id) => match self.refs.get(id.0 as usize) {
                Some(body) => match self.read.get(&(id.0, cx)) {
                    Some(slot) => *slot,
                    None => {
                        let slot = self.lower(body, cx);
                        self.read.insert((id.0, cx), slot);
                        slot
                    }
                },
                None => self.fail(CollapseError::NotEvaluable("a node")),
            },
            Body::Run(run) => self.push(Op::Run {
                time: cx.time,
                run: run.clone(),
            }),
            Body::Index(i) => match self.bound.iter().position(|j| j == i) {
                Some(k) => self.push(Op::Bound(k)),
                None => self.fail(CollapseError::NotEvaluable(sketch(f))),
            },
            Body::Banded(b) => {
                let mut bound = self.bound.clone();
                bound.push(b.series.index);
                let term = Program::bound(
                    &[&b.series.term.body],
                    self.refs,
                    (cx.component, cx.on),
                    bound,
                );
                self.push(Op::Banded {
                    time: cx.time,
                    on: cx.on,
                    banded: b.clone(),
                    term: Box::new(term),
                })
            }
            other => self.fail(CollapseError::NotEvaluable(sketch(other))),
        }
    }
}

/// The value operands of an op that reads nothing but them; empty for any other.
fn operands(op: &Op) -> Vec<usize> {
    match op {
        Op::Add(p) | Op::Mul(p) | Op::Fold(_, p) => p.clone(),
        Op::Div(a, b) => vec![*a, *b],
        Op::Pow(a, _) | Op::Apply(_, a) | Op::Keyed { of: a, .. } => vec![*a],
        _ => Vec::new(),
    }
}

/// `op` reading `to` in place of its operands, in `operands`' order.
fn with_operands(op: Op, to: &[usize]) -> Op {
    match op {
        Op::Add(_) => Op::Add(to.to_vec()),
        Op::Mul(_) => Op::Mul(to.to_vec()),
        Op::Fold(f, _) => Op::Fold(f, to.to_vec()),
        Op::Div(..) => Op::Div(to[0], to[1]),
        Op::Pow(_, n) => Op::Pow(to[0], n),
        Op::Apply(f, _) => Op::Apply(f, to[0]),
        Op::Keyed { seed, .. } => Op::Keyed { seed, of: to[0] },
        other => other,
    }
}

/// What an op computes, numbers by their bits; `None` for one never shared.
fn key(op: &Op) -> Option<Vec<u64>> {
    let slots = |tag: u64, s: &[usize]| {
        std::iter::once(tag)
            .chain(s.iter().map(|s| *s as u64))
            .collect::<Vec<u64>>()
    };
    Some(match op {
        Op::Time => vec![0],
        Op::Const(c) => vec![1, c.re.to_bits(), c.im.to_bits()],
        Op::Line(t) => vec![2, *t as u64],
        Op::Shifted(t, by) => vec![3, *t as u64, by.to_bits()],
        Op::Re(x) => vec![4, *x as u64],
        Op::Warp { when, of } => vec![5, *when as u64, *of as u64],
        Op::Add(p) => slots(6, p),
        Op::Mul(p) => slots(7, p),
        Op::Div(a, b) => vec![8, *a as u64, *b as u64],
        Op::Pow(a, n) => vec![9, *a as u64, *n as u64],
        Op::Apply(f, a) => vec![10, *f as u64, *a as u64],
        Op::Fold(f, p) => [vec![11, *f as u64], slots(0, p)].concat(),
        Op::Crop {
            time,
            on,
            of,
            l,
            r,
            rise,
            fall,
        } => vec![
            12,
            *time as u64,
            u64::from(*on),
            *of as u64,
            l.to_bits(),
            r.to_bits(),
            rise.to_bits(),
            fall.to_bits(),
        ],
        Op::Singular { time, order } => vec![13, *time as u64, *order as u64],
        Op::Keyed { seed, of } => vec![14, *seed, *of as u64],
        Op::Bound(k) => vec![15, *k as u64],
        Op::Fail(_) | Op::Modal { .. } | Op::Run { .. } | Op::Banded { .. } => return None,
    })
}

/// One slot's values over the lanes, and each lane's refusal once any lane refused.
#[derive(Clone, Default)]
struct Col {
    v: Vec<C64>,
    err: Vec<Option<CollapseError>>,
}

impl Col {
    fn err(&self, i: usize) -> Option<&CollapseError> {
        match self.err.is_empty() {
            true => None,
            false => self.err[i].as_ref(),
        }
    }

    fn fail(&mut self, i: usize, e: CollapseError) {
        if self.err.is_empty() {
            self.err.resize(self.v.len(), None);
        }
        self.v[i] = C64::ZERO;
        self.err[i] = Some(e);
    }

    fn set(&mut self, i: usize, v: Result<C64, CollapseError>) {
        match v {
            Ok(v) if v.is_finite() => self.v[i] = v,
            Ok(_) => self.fail(i, INFINITE),
            Err(e) => self.fail(i, e),
        }
    }

    fn get(&self, i: usize) -> Result<C64, &CollapseError> {
        match self.err(i) {
            Some(e) => Err(e),
            None => Ok(self.v[i]),
        }
    }
}

/// A program's slots over one column of lanes, each evaluated once a reader needs it, only
/// over the lanes it needs.
pub(crate) struct Columns<'p> {
    program: &'p Program,
    /// Each lane's instant, and its sample where it is one.
    pub(crate) t: Vec<f64>,
    pub(crate) on: Vec<Option<i64>>,
    pub(crate) grid: Grid,
    bound: Vec<f64>,
    cols: Vec<Col>,
    done: Vec<(usize, usize)>,
    lanes: usize,
}

impl Columns<'_> {
    /// Forgets every value, to be run again at the instants written since.
    pub(crate) fn again(&mut self) {
        self.done.iter_mut().for_each(|d| *d = (0, 0));
        self.cols.iter_mut().for_each(|c| c.err.clear());
    }

    /// Root `k` over lanes `[lo, hi)`: each lane's value or its refusal.
    pub(crate) fn root(&mut self, k: usize, (lo, hi): (usize, usize)) -> RootView<'_> {
        let slot = self.program.roots[k];
        self.need(slot, lo, hi);
        RootView {
            col: &self.cols[slot],
        }
    }

    fn need(&mut self, s: usize, lo: usize, hi: usize) {
        if lo >= hi {
            return;
        }
        let (dlo, dhi) = self.done[s];
        if dlo < dhi && dlo <= lo && hi <= dhi {
            return;
        }
        let mut col = std::mem::take(&mut self.cols[s]);
        if col.v.len() != self.lanes {
            col.v.resize(self.lanes, C64::ZERO);
        }
        let held = match dlo < dhi {
            true => {
                if lo < dlo {
                    self.piece(s, &mut col, lo, dlo);
                }
                if dhi < hi {
                    self.piece(s, &mut col, dhi, hi);
                }
                (lo.min(dlo), hi.max(dhi))
            }
            false => {
                self.piece(s, &mut col, lo, hi);
                (lo, hi)
            }
        };
        self.cols[s] = col;
        self.done[s] = held;
    }

    /// Whether lane `i` stands at a sample, for an op that may read one.
    fn sample(&self, on: bool, i: usize) -> Option<(Grid, i64)> {
        match on {
            true => self.on[i].map(|n| (self.grid, n)),
            false => None,
        }
    }

    fn piece(&mut self, s: usize, out: &mut Col, a: usize, b: usize) {
        let program = self.program;
        match &program.ops[s] {
            Op::Time => {
                for i in a..b {
                    out.v[i] = C64::real(self.t[i]);
                }
            }
            Op::Const(c) => out.v[a..b].fill(*c),
            Op::Fail(e) => (a..b).for_each(|i| out.fail(i, e.clone())),
            Op::Line(t) => {
                self.need(*t, a, b);
                let t = &self.cols[*t];
                for i in a..b {
                    out.set(i, Ok(C64::real(t.v[i].re)));
                }
            }
            Op::Shifted(t, by) => {
                self.need(*t, a, b);
                let t = &self.cols[*t];
                for i in a..b {
                    out.v[i] = C64::real(t.v[i].re - by);
                }
            }
            Op::Re(x) => {
                self.need(*x, a, b);
                let x = &self.cols[*x];
                for i in a..b {
                    match x.err(i) {
                        Some(e) => {
                            out.fail(i, e.clone());
                            out.v[i] = C64::ZERO;
                        }
                        None => out.v[i] = C64::real(x.v[i].re),
                    }
                }
            }
            Op::Warp { when, of } => {
                self.need(*when, a, b);
                self.need(*of, a, b);
                let (when, of) = (&self.cols[*when], &self.cols[*of]);
                for i in a..b {
                    match when.err(i) {
                        Some(e) => out.fail(i, e.clone()),
                        None => out.set(i, of.get(i).map_err(Clone::clone)),
                    }
                }
            }
            Op::Add(parts) => {
                parts.iter().for_each(|p| self.need(*p, a, b));
                let clean = parts.iter().all(|p| self.cols[*p].err.is_empty());
                match clean {
                    true => {
                        out.v[a..b].fill(C64::ZERO);
                        for p in parts {
                            let p = &self.cols[*p].v[a..b];
                            for (o, x) in out.v[a..b].iter_mut().zip(p) {
                                *o = *o + *x;
                            }
                        }
                        for i in a..b {
                            if !out.v[i].is_finite() {
                                out.fail(i, INFINITE);
                            }
                        }
                    }
                    false => {
                        for i in a..b {
                            let sum = parts
                                .iter()
                                .try_fold(C64::ZERO, |held, p| Ok(held + self.cols[*p].get(i)?));
                            out.set(i, sum.map_err(|e: &CollapseError| e.clone()));
                        }
                    }
                }
            }
            Op::Mul(parts) => self.product(parts, out, a, b),
            Op::Div(x, y) => {
                self.need(*x, a, b);
                self.need(*y, a, b);
                let (x, y) = (&self.cols[*x], &self.cols[*y]);
                for i in a..b {
                    let v = x.get(i).and_then(|x| Ok(x / y.get(i)?));
                    out.set(i, v.map_err(Clone::clone));
                }
            }
            Op::Pow(x, n) => self.map(*x, out, (a, b), |x| power(x, *n)),
            Op::Apply(f, x) => self.map(*x, out, (a, b), |x| unary(*f, x)),
            Op::Fold(f, parts) => {
                parts.iter().for_each(|p| self.need(*p, a, b));
                for i in a..b {
                    let mut each = parts.iter().map(|p| self.cols[*p].get(i));
                    let first = each.next().expect("a fold holds one part");
                    let v = first.and_then(|first| {
                        each.try_fold(first, |acc, v| {
                            let v = v?;
                            Ok(C64::real(match f {
                                Fold::Max => acc.re.max(v.re),
                                Fold::Min => acc.re.min(v.re),
                                Fold::Mod => acc.re.rem_euclid(v.re),
                            }))
                        })
                    });
                    out.set(i, v.map_err(Clone::clone));
                }
            }
            Op::Crop {
                time,
                on,
                of,
                l,
                r,
                rise,
                fall,
            } => {
                self.need(*time, a, b);
                let gains: Vec<f64> = (a..b)
                    .map(|i| {
                        let t = self.cols[*time].v[i].re;
                        match self.sample(*on, i) {
                            Some((grid, n)) if grid.inside(n, *l, *r) => {
                                shoulders(t, *l, *r, *rise, *fall)
                            }
                            Some(_) => 0.0,
                            None => crop_gain(t, *l, *r, *rise, *fall),
                        }
                    })
                    .collect();
                let open = |g: &f64| *g != 0.0;
                let (lo, hi) = match gains.iter().position(open) {
                    Some(lo) => (lo, gains.iter().rposition(open).expect("one opens") + 1),
                    None => (0, 0),
                };
                self.need(*of, a + lo, a + hi);
                let of = &self.cols[*of];
                for (i, gain) in (a..b).zip(gains) {
                    match gain == 0.0 {
                        true => out.v[i] = C64::ZERO,
                        false => out.set(i, of.get(i).map(|v| v.scale(gain)).map_err(Clone::clone)),
                    }
                }
            }
            Op::Singular { time, order } => {
                self.need(*time, a, b);
                for i in a..b {
                    let at = self.cols[*time].v[i].re;
                    out.fail(i, CollapseError::SingularInCt { at, order: *order });
                }
            }
            Op::Keyed { seed, of } => self.map_or(*of, out, (a, b), |key| {
                Ok(C64::real(sva_formula::draw_nearest(*seed, key.re).ok_or(
                    CollapseError::NotEvaluable("a key past any step"),
                )?))
            }),
            Op::Modal { time, on, atoms } => {
                self.need(*time, a, b);
                for i in a..b {
                    let (t, at) = (self.cols[*time].v[i].re, self.sample(*on, i));
                    let sum = atoms
                        .iter()
                        .try_fold(C64::ZERO, |sum, atom| Ok(sum + eval_atom_on(atom, t, at)?));
                    out.set(i, sum);
                }
            }
            Op::Run { time, run } => {
                self.need(*time, a, b);
                for i in a..b {
                    out.set(i, Ok(super::run::at(run, self.cols[*time].v[i].re)));
                }
            }
            Op::Bound(k) => out.v[a..b].fill(C64::real(self.bound[*k])),
            Op::Banded {
                time,
                on,
                banded,
                term,
            } => self.banded((*time, *on), banded, term, out, (a, b)),
        }
    }

    fn map(&mut self, x: usize, out: &mut Col, (a, b): (usize, usize), f: impl Fn(C64) -> C64) {
        self.map_or(x, out, (a, b), |x| Ok(f(x)));
    }

    fn map_or(
        &mut self,
        x: usize,
        out: &mut Col,
        (a, b): (usize, usize),
        f: impl Fn(C64) -> Result<C64, CollapseError>,
    ) {
        self.need(x, a, b);
        let x = &self.cols[x];
        for i in a..b {
            match x.get(i) {
                Ok(v) => out.set(i, f(v)),
                Err(e) => out.fail(i, e.clone()),
            }
        }
    }

    /// A factor reading `+0` exactly zeroes the product whatever any other refused, so a
    /// factor is evaluated only over the lanes no factor before it zeroed.
    fn product(&mut self, parts: &[usize], out: &mut Col, a: usize, b: usize) {
        let mut zeroed = vec![false; b - a];
        let (mut lo, mut hi) = (a, b);
        for p in parts {
            self.need(*p, lo, hi);
            let col = &self.cols[*p];
            for i in lo..hi {
                if col.err(i).is_none() && col.v[i].is_zero() {
                    zeroed[i - a] = true;
                }
            }
            while lo < hi && zeroed[lo - a] {
                lo += 1;
            }
            while lo < hi && zeroed[hi - 1 - a] {
                hi -= 1;
            }
        }
        for i in a..b {
            if zeroed[i - a] {
                out.v[i] = C64::ZERO;
                continue;
            }
            let mut held: Result<C64, &CollapseError> = Ok(C64::ONE);
            for p in parts {
                match (self.cols[*p].get(i), &held) {
                    (Ok(v), Ok(acc)) => held = Ok(*acc * v),
                    (Err(e), Ok(_)) => held = Err(e),
                    (_, Err(_)) => {}
                }
            }
            out.set(i, held.map_err(Clone::clone));
        }
    }

    /// The terms whose carrier turns under the ceiling at each lane's instant, summed from +0
    /// in index order, each index's term evaluated once over the lanes that sum it.
    fn banded(
        &mut self,
        (time, on): (usize, bool),
        b: &Banded,
        term: &Program,
        out: &mut Col,
        (a, z): (usize, usize),
    ) {
        self.need(time, a, z);
        let mut ranges: Vec<Option<(i64, i64)>> = vec![None; z - a];
        for i in a..z {
            let (t, at) = (self.cols[time].v[i].re, self.sample(on, i));
            let turned =
                |rate: &sva_formula::SpectralSum| Ok(eval_lane_on(&rate.lanes[0], t, at)?.re);
            let within: Result<_, CollapseError> =
                (|| Ok(b.within(turned(&b.slope)?, turned(&b.offset)?)))();
            match within {
                Ok(Some(range)) => {
                    ranges[i - a] = Some(range);
                    out.v[i] = C64::ZERO;
                }
                Ok(None) => out.v[i] = C64::ZERO,
                Err(e) => out.fail(i, e),
            }
        }
        let mut spans: Vec<(i64, i64)> = ranges.iter().flatten().copied().collect();
        if spans.is_empty() {
            return;
        }
        spans.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::with_capacity(spans.len());
        for (from, to) in spans {
            match merged.last_mut() {
                Some(held) if from <= held.1.saturating_add(1) => held.1 = held.1.max(to),
                _ => merged.push((from, to)),
            }
        }
        let mut inner = term.columns(z - a);
        for i in a..z {
            inner.t[i - a] = self.cols[time].v[i].re;
            inner.on[i - a] = self.on[i];
        }
        inner.grid = self.grid;
        inner.bound = self.bound.clone();
        inner.bound.push(0.0);
        let mut summing: Vec<bool> = ranges.iter().map(Option::is_some).collect();
        for k in merged.into_iter().flat_map(|(from, to)| from..=to) {
            let holds = |summing: &[bool], j: usize| {
                summing[j] && ranges[j].is_some_and(|(f, t)| f <= k && k <= t)
            };
            let Some(lo) = (0..z - a).find(|j| holds(&summing, *j)) else {
                continue;
            };
            let hi = (0..z - a)
                .rfind(|j| holds(&summing, *j))
                .expect("one holds")
                + 1;
            inner.again();
            *inner.bound.last_mut().expect("the index pushed") = k as f64;
            let summed = inner.root(0, (lo, hi));
            for j in lo..hi {
                if !holds(&summing, j) {
                    continue;
                }
                match summed.get(j) {
                    Ok(v) => out.v[a + j] = out.v[a + j] + v,
                    Err(e) => {
                        out.fail(a + j, e.clone());
                        summing[j] = false;
                    }
                }
            }
        }
        for i in a..z {
            if out.err(i).is_none() && !out.v[i].is_finite() {
                out.fail(i, INFINITE);
            }
        }
    }
}

/// One root's lanes, each its value or why it has none.
pub(crate) struct RootView<'c> {
    col: &'c Col,
}

impl RootView<'_> {
    pub(crate) fn get(&self, i: usize) -> Result<C64, &CollapseError> {
        self.col.get(i)
    }
}

fn power(x: C64, n: i32) -> C64 {
    match n {
        0.. => x.powi(n as u32),
        _ => x.powi(n.unsigned_abs()).inv(),
    }
}

/// The component count a written subterm answers for. Only `join` widens, and only `ch` and
/// a node narrow back.
/// `refs[k]` the width of the form `Body::Node(k)` names; one past it is one wide.
pub(crate) fn width_of(f: &Body, refs: &[usize]) -> usize {
    match f {
        Body::Join(parts) => parts.iter().map(|p| width_of(&p.body, refs)).sum(),
        Body::Channel(..) => 1,
        Body::Node(id) => refs.get(id.0 as usize).map_or(1, |w| (*w).max(1)),
        other => sva_formula::closed_form::children(other)
            .iter()
            .map(|p| width_of(&p.body, refs))
            .max()
            .unwrap_or(1),
    }
}

fn sketch(f: &Body) -> &'static str {
    match f {
        Body::Param(_) => "an unsubstituted parameter",
        Body::Index(_) => "a free series index",
        Body::Deriv { .. } => "a derivative",
        Body::Rational(_) => "a rational",
        Body::Series(_) => "a series",
        _ => "this subterm",
    }
}
