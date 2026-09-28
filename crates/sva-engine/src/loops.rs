// Concern: classifies a self-reference, and folds a read's time or a number to its exact value | Non-concern: running a loop (sva-samples), lowering (lower/) | IO: (body, Cx) -> SelfKind, Affine

use sva_ast::{Arg, BinOp, ByteSpan, Expr, Literal};
use sva_formula::closed_form::{map_children, read_at};
use sva_formula::{Body, C64, IndexId, Part, Series, Var};

use crate::arguments::Chosen;
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::{Cx, Instances, Node};
use crate::time::{Affine, Q};
use crate::typing::Gain;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SelfKind {
    Series {
        gain: C64,
        delay: f64,
    },
    /// Run on the lattice; `gain` bounds how far the output moves per unit its taps move,
    /// where the body is linear in them, and `taps` names each one where all are fixed.
    Sampled {
        gain: Option<Gain>,
        taps: Option<Vec<(f64, Q)>>,
    },
    Refuse(Box<EngineError>),
}

/// Linear in one `self` a constant delay back with `abs(g) < 1` is a series. A loop written on
/// the lattice, which `sp` in its body says, runs there; there a running sum is a value.
pub(crate) fn classify(inst: &Instances, e: &Expr, cx: Cx, at: &str, stepped: bool) -> SelfKind {
    match read(inst, e, cx, C64::ONE) {
        Reading::Refused(tap) => SelfKind::Refuse(Box::new(
            tap_refusal(tap, Located::at(at, None)).expect("a refused tap names its reason"),
        )),
        Reading::Free | Reading::Nonlinear => SelfKind::Sampled {
            gain: None,
            taps: None,
        },
        Reading::Linear { taps, plain, norm } => {
            let fixed = |taps: &[(C64, Tap)]| {
                taps.iter()
                    .map(|(g, tap)| match tap {
                        Tap::Back(d) if g.im == 0.0 => Some((g.re, *d)),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()
            };
            match (plain, taps.as_slice()) {
                (true, [(g, Tap::Back(delay))]) if !stepped && g.abs() < 1.0 => SelfKind::Series {
                    gain: *g,
                    delay: delay.to_f64(),
                },
                (true, [(g, Tap::Back(_))]) if !stepped || g.abs() > 1.0 => {
                    SelfKind::Refuse(Box::new(unbounded(*g, at)))
                }
                _ => SelfKind::Sampled {
                    gain: Some(norm),
                    taps: plain.then(|| fixed(&taps)).flatten(),
                },
            }
        }
    }
}

/// What one `self(...)` call site reads: a constant delay back, a time that moves, or the
/// reason it reads nothing already written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Tap {
    Back(Q),
    Moving,
    Zero,
    Forward,
}

/// The one reading of a self-reference's time, so classification and lowering cannot drift.
pub(crate) fn tap_of(inst: &Instances, arg: &Expr, cx: Cx) -> Tap {
    let Some(time) = time_of(inst, arg, cx) else {
        return Tap::Moving;
    };
    if time.scale != Q::ONE {
        return Tap::Moving;
    }
    match time.shift.neg() {
        d if d.is_zero() => Tap::Zero,
        d if d > Q::ZERO => Tap::Back(d),
        _ => Tap::Forward,
    }
}

pub(crate) fn tap_refusal(tap: Tap, at: Located) -> Option<EngineError> {
    let (code, message, help) = match tap {
        Tap::Back(_) | Tap::Moving => return None,
        Tap::Zero => (
            "samples.zero_delay_loop",
            "a loop reaches no sample it has already written.",
            "write self(t - 1sp) for a one-step loop",
        ),
        Tap::Forward => (
            "engine.forward_self_read",
            "a loop reads its own output before it is written.",
            "write self at an earlier time, as in self(t - 1sp)",
        ),
    };
    Some(EngineError::refused(Diagnostic {
        code: code.to_string(),
        message: message.to_string(),
        location: at,
        help: help.to_string(),
    }))
}

fn unbounded(gain: C64, at: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "type.self_gain_unbounded".to_string(),
        message: format!("loop gain {} does not settle.", gain.abs()),
        location: Located::at(at, None),
        help: "write self(t - 1sp) for a sampled loop".to_string(),
    })
}

/// What one subterm gives: nothing, scaled reads of `self`, or a shape no gain bounds. A
/// component taken or joined keeps a gain but no longer spells one series.
enum Reading {
    Free,
    /// `norm` bounds the output's move per unit move of every tap, component by component.
    Linear {
        taps: Vec<(C64, Tap)>,
        plain: bool,
        norm: Gain,
    },
    Refused(Tap),
    Nonlinear,
}

fn read(inst: &Instances, e: &Expr, cx: Cx, gain: C64) -> Reading {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| read(inst, e2, cx2, gain)) {
        return r;
    }
    match inst.node(e, cx) {
        Node::Own { arg, .. } => match tap_of(inst, arg, cx) {
            tap @ (Tap::Back(_) | Tap::Moving) => Reading::Linear {
                taps: vec![(gain, tap)],
                plain: true,
                norm: tap_gain(tap, gain.abs()),
            },
            refused => Reading::Refused(refused),
        },
        Node::Bin(op @ (BinOp::Add | BinOp::Sub), l, r) => {
            let right = if op == BinOp::Sub { -gain } else { gain };
            join(read(inst, l, cx, gain), read(inst, r, cx, right))
        }
        Node::Bin(BinOp::Mul, l, r) => match (holds(inst, l, cx), holds(inst, r, cx)) {
            (false, true) => match constant(inst, l, cx) {
                Some(k) => read(inst, r, cx, gain * k),
                None => Reading::Nonlinear,
            },
            (true, false) => match constant(inst, r, cx) {
                Some(k) => read(inst, l, cx, gain * k),
                None => Reading::Nonlinear,
            },
            (false, false) => Reading::Free,
            (true, true) => Reading::Nonlinear,
        },
        Node::Bin(BinOp::Div, l, r) => match (holds(inst, l, cx), holds(inst, r, cx)) {
            (true, false) => match constant(inst, r, cx) {
                Some(k) if !k.is_zero() => read(inst, l, cx, gain / k),
                _ => Reading::Nonlinear,
            },
            (false, false) => Reading::Free,
            _ => Reading::Nonlinear,
        },
        Node::Call { name, args, .. } if name == crate::vocabulary::CHANNEL => match args {
            [Arg::Pos(x), Arg::Pos(k)] if !holds(inst, k, cx) => opaque(read(inst, x, cx, gain)),
            _ => Reading::Nonlinear,
        },
        Node::Call { name, args, .. } if name == crate::vocabulary::JOIN => args
            .iter()
            .map(|a| match a {
                Arg::Pos(x) => opaque(read(inst, x, cx, gain)),
                Arg::Named(..) => Reading::Nonlinear,
            })
            .fold(Reading::Free, beside),
        other @ Node::Call { name, args, .. } => match holds_in(inst, &other, cx) {
            true => called_on_self(inst, (name, args), cx, gain),
            false => Reading::Free,
        },
        other => match holds_in(inst, &other, cx) {
            true => Reading::Nonlinear,
            false => Reading::Free,
        },
    }
}

/// A call over the loop's own past, bounded by how far its output moves per unit its signal
/// does: a fixed filter by its impulse response's sum, a crop by one, and the Lipschitz
/// constant of each unary this knows one for. Any other stays unbounded.
fn called_on_self(inst: &Instances, (name, args): (&str, &[Arg]), cx: Cx, gain: C64) -> Reading {
    let positional: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            Arg::Pos(x) => Some(x),
            Arg::Named(..) => None,
        })
        .collect();
    let written = |slot: usize, key: &str| {
        positional.get(slot).copied().or_else(|| {
            args.iter().find_map(|a| match a {
                Arg::Named(k, x) if k == key => Some(x),
                _ => None,
            })
        })
    };
    let fixed = |x: Option<&Expr>, fallback: f64| match x {
        Some(x) if !holds(inst, x, cx) => {
            constant(inst, x, cx).filter(|c| c.im == 0.0).map(|c| c.re)
        }
        Some(_) => None,
        None => Some(fallback),
    };
    let Some(&signal) = positional.first() else {
        return Reading::Nonlinear;
    };
    let others_free = |from: usize| positional[from..].iter().all(|x| !holds(inst, x, cx));
    let (by, lti) = match name {
        _ if let Some(shape) = sva_formula::filter::Shape::from_name(name) => {
            let parameters = (
                fixed(written(1, "cutoff"), f64::NAN),
                fixed(written(2, "q"), shape.default_q()),
                fixed(written(3, "gain"), 0.0),
            );
            let (Some(cutoff), Some(q), Some(db)) = parameters else {
                return Reading::Nonlinear;
            };
            let rate = f64::from(lattice());
            let (coeffs, _) = sva_samples::filters::coefficients(shape, cutoff, q, db, rate);
            match cutoff
                .is_finite()
                .then(|| sva_samples::gain::l1(&coeffs))
                .flatten()
            {
                Some(l1) => (l1, true),
                None => return Reading::Nonlinear,
            }
        }
        "crop" if others_free(1) => (1.0, false),
        "tanh" | "sin" | "cos" | "abs" if positional.len() == 1 => (1.0, false),
        "sat" if positional.len() == 1 => match fixed(written(1, "drive"), 1.0) {
            Some(drive) => (drive.abs(), false),
            None => return Reading::Nonlinear,
        },
        "max" | "min" => {
            return positional
                .iter()
                .map(|x| scaled(opaque(read(inst, x, cx, gain)), 1.0, false))
                .fold(Reading::Free, beside);
        }
        _ => return Reading::Nonlinear,
    };
    scaled(opaque(read(inst, signal, cx, gain)), by, lti)
}

/// Its gain times `by`, and no longer time-invariant unless `lti`.
fn scaled(r: Reading, by: f64, lti: bool) -> Reading {
    match r {
        Reading::Linear { taps, plain, norm } => Reading::Linear {
            taps,
            plain,
            norm: norm.scaled(by, lti),
        },
        other => other,
    }
}

fn opaque(r: Reading) -> Reading {
    match r {
        Reading::Linear { taps, norm, .. } => Reading::Linear {
            taps,
            plain: false,
            norm,
        },
        other => other,
    }
}

/// Two components side by side: each moves by its own taps alone.
fn beside(a: Reading, b: Reading) -> Reading {
    let norms = |r: &Reading| match r {
        Reading::Linear { norm, .. } => *norm,
        _ => Gain::default(),
    };
    let widest = norms(&a).widest(norms(&b));
    match join(a, b) {
        Reading::Linear { taps, plain, .. } => Reading::Linear {
            taps,
            plain,
            norm: widest,
        },
        other => other,
    }
}

fn join(a: Reading, b: Reading) -> Reading {
    match (a, b) {
        (Reading::Refused(tap), _) | (_, Reading::Refused(tap)) => Reading::Refused(tap),
        (Reading::Nonlinear, _) | (_, Reading::Nonlinear) => Reading::Nonlinear,
        (Reading::Free, other) | (other, Reading::Free) => other,
        (
            Reading::Linear {
                taps: mut held,
                plain: p1,
                norm: n1,
            },
            Reading::Linear {
                taps: more,
                plain: p2,
                norm: n2,
            },
        ) => {
            for (g, tap) in more {
                match held
                    .iter_mut()
                    .find(|(_, t)| *t == tap && tap != Tap::Moving)
                {
                    Some((sum, _)) => *sum = *sum + g,
                    None => held.push((g, tap)),
                }
            }
            Reading::Linear {
                taps: held,
                plain: p1 && p2,
                norm: n1.plus(n2),
            }
        }
    }
}

fn holds(inst: &Instances, e: &Expr, cx: Cx) -> bool {
    holds_in(inst, &inst.node(e, cx), cx) || inst.holds_self(e, cx)
}

fn holds_in(inst: &Instances, node: &Node, cx: Cx) -> bool {
    match node {
        Node::Own { .. } => true,
        Node::Read { arg, .. } => inst.holds_self(arg, cx),
        Node::Call { args, .. } => args.iter().any(|a| {
            let (sva_ast::Arg::Pos(x) | sva_ast::Arg::Named(_, x)) = a;
            inst.holds_self(x, cx)
        }),
        Node::Bin(_, l, r) => inst.holds_self(l, cx) || inst.holds_self(r, cx),
        Node::Lit(_) | Node::Name(_) => false,
    }
}

fn constant(inst: &Instances, e: &Expr, cx: Cx) -> Option<C64> {
    plain(amount(inst, e, cx)?).map(C64::real)
}

/// A linear loop whose fractional taps fall inside the kernel's reach, each such tap
/// replaced by the loop's own equation one delay back until every tap on its own past clears
/// it: the reads of the rest and of its past, each `(delay, coefficient)`.
pub(crate) enum Expanded {
    Needless,
    Taps {
        rest: Vec<(Q, f64)>,
        own: Vec<(Q, f64)>,
    },
    Unreachable,
}

const EXPANSIONS: usize = 256;

pub(crate) fn expanded(taps: &[(f64, Q)], half: usize) -> Expanded {
    let lattice = Q::int(i64::from(lattice()));
    let reach = Q::int(half as i64);
    let short = |d: &Q| {
        d.mul(lattice)
            .is_some_and(|samples| !samples.is_integer() && samples <= reach)
    };
    if !taps.iter().any(|(_, d)| short(d)) {
        return Expanded::Needless;
    }
    let mut rest = std::collections::BTreeMap::from([(Q::ZERO, 1.0)]);
    let mut own: std::collections::BTreeMap<Q, f64> = std::collections::BTreeMap::new();
    for (g, d) in taps {
        *own.entry(*d).or_insert(0.0) += g;
    }
    for _ in 0..EXPANSIONS {
        let Some((&d, &c)) = own.iter().find(|(d, _)| short(d)) else {
            return Expanded::Taps {
                rest: rest.into_iter().collect(),
                own: own.into_iter().collect(),
            };
        };
        own.remove(&d);
        *rest.entry(d).or_insert(0.0) += c;
        for (g, e) in taps {
            match d.add(*e) {
                Some(at) => *own.entry(at).or_insert(0.0) += c * g,
                None => return Expanded::Unreachable,
            }
        }
    }
    Expanded::Unreachable
}

/// `sum(k, 0, inf, g^k * rest(t - k*d))`, with the shift written into the body and the
/// product distributed, so each addend of the body is one readable wave.
pub(crate) fn neumann(rest: &Body, gain: C64, delay: f64, index: IndexId) -> Body {
    let log = C64::new(gain.abs().ln(), gain.im.atan2(gain.re));
    let power = Body::Apply(
        sva_formula::Unary::Exp,
        Part::bare(Body::Mul(vec![
            Part::bare(Body::Index(index)),
            Part::bare(Body::Const(log)),
        ])),
    );
    let addends: Vec<&Body> = match rest {
        Body::Add(parts) => parts.iter().map(|p| &*p.body).collect(),
        other => vec![other],
    };
    let scaled: Vec<Part> = addends
        .into_iter()
        .map(|addend| {
            Part::bare(Body::Mul(vec![
                Part::bare(power.clone()),
                Part::bare(moved(addend, index, delay)),
            ]))
        })
        .collect();
    let term = match scaled.as_slice() {
        [only] => (*only.body).clone(),
        _ => Body::Add(scaled),
    };
    Body::Series(Box::new(Series {
        index,
        lo: 0,
        hi: sva_formula::Bound::Infinite,
        term: Part::bare(term),
    }))
}

/// `t -> t - k*d`, windows and all.
fn moved(f: &Body, index: IndexId, delay: f64) -> Body {
    let at = Body::Add(vec![
        Part::bare(Body::Line),
        Part::bare(Body::Mul(vec![
            Part::bare(Body::Const(C64::real(-delay))),
            Part::bare(Body::Index(index)),
        ])),
    ]);
    read_at(f, &at)
}

/// A series term holds the body inline, so a node left inside it would normalize to nothing.
pub(crate) fn expandable(
    f: &Body,
    var: Var,
    of: &dyn Fn(sva_formula::NodeId) -> Option<(Body, Var)>,
) -> Option<Body> {
    match f {
        Body::Node(id) => {
            let (body, held) = of(*id)?;
            match held == var {
                true => expandable(&body, var, of),
                false => None,
            }
        }
        other => {
            let mut ok = true;
            let out = map_children(other, |p| match expandable(&p.body, var, of) {
                Some(body) => Part::new(p.origin, body),
                None => {
                    ok = false;
                    p.clone()
                }
            });
            ok.then_some(out)
        }
    }
}

/// `t` scaled by a constant and moved by constants, each exact; `None` where the time is no
/// such line, or where a constant in it has no exact value.
pub fn time_of(inst: &Instances, e: &Expr, cx: Cx) -> Option<Affine> {
    match walk(inst, e, cx)? {
        Term::Line(line) => Some(line),
        Term::Number(shift) => Some(Affine {
            scale: Q::ZERO,
            shift,
        }),
    }
}

/// A term of a time: a line in `t`, or one exact number.
enum Term {
    Line(Affine),
    Number(Q),
}

impl Term {
    fn parts(&self) -> (Q, Q) {
        match self {
            Term::Line(a) => (a.scale, a.shift),
            Term::Number(q) => (Q::ZERO, *q),
        }
    }

    fn of(scale: Q, shift: Q) -> Term {
        match scale.is_zero() {
            true => Term::Number(shift),
            false => Term::Line(Affine { scale, shift }),
        }
    }
}

fn walk(inst: &Instances, e: &Expr, cx: Cx) -> Option<Term> {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| walk(inst, e2, cx2)) {
        return r;
    }
    match inst.node(e, cx) {
        Node::Name("t") => Some(Term::Line(Affine::NOW)),
        Node::Bin(op @ (BinOp::Add | BinOp::Sub), l, r) => {
            let ((a, b), (c, d)) = (walk(inst, l, cx)?.parts(), walk(inst, r, cx)?.parts());
            let (c, d) = match op {
                BinOp::Sub => (c.neg(), d.neg()),
                _ => (c, d),
            };
            Some(Term::of(a.add(c)?, b.add(d)?))
        }
        Node::Bin(BinOp::Mul, l, r) => match (walk(inst, l, cx)?, walk(inst, r, cx)?) {
            (Term::Number(k), other) | (other, Term::Number(k)) => {
                let (a, b) = other.parts();
                Some(Term::of(a.mul(k)?, b.mul(k)?))
            }
            _ => None,
        },
        Node::Bin(BinOp::Div, l, r) => {
            let Term::Number(by) = walk(inst, r, cx)? else {
                return None;
            };
            let (a, b) = walk(inst, l, cx)?.parts();
            Some(Term::of(a.div(by)?, b.div(by)?))
        }
        Node::Bin(BinOp::Mod, l, r) => match (walk(inst, l, cx)?, walk(inst, r, cx)?) {
            (Term::Number(a), Term::Number(b)) => Some(Term::Number(a.rem(b)?)),
            _ => None,
        },
        Node::Lit(Literal::Num(n)) => Q::decimal(*n).map(Term::Number),
        Node::Lit(Literal::Samples(n)) => Some(Term::Number(
            Q::decimal(*n)?.div(Q::int(i64::from(lattice())))?,
        )),
        _ => Q::decimal(plain(amount(inst, e, cx)?)?).map(Term::Number),
    }
}

/// A tap's gain `g`, whole where it reads a lattice sample and through the kernel elsewhere.
pub(crate) fn tap_gain(tap: Tap, g: f64) -> Gain {
    match tap {
        Tap::Back(d) if on_lattice(d) => Gain {
            whole: g,
            kernel: 0.0,
            lti: true,
        },
        _ => Gain {
            whole: 0.0,
            kernel: g,
            lti: tap != Tap::Moving,
        },
    }
}

/// Fixed taps on a loop's own past, each `(delay, coefficient)`.
pub(crate) fn own_gain(own: &[(Q, f64)]) -> Gain {
    own.iter().fold(Gain::default(), |held, (d, c)| {
        held.plus(tap_gain(Tap::Back(*d), c.abs()))
    })
}

/// A delay of whole lattice steps, which a loop reads with no kernel.
pub(crate) fn on_lattice(delay: Q) -> bool {
    delay
        .mul(Q::int(i64::from(lattice())))
        .is_some_and(|samples| samples.is_integer())
}

/// The step every stateful node runs at, which `1sp` is one of.
pub(crate) fn lattice() -> u32 {
    sva_samples::PSYCHOACOUSTIC_V1.lattice_hz
}

/// A written number over every operator FORMAT 3.3 folds. What this drops is defaulted, never
/// refused.
pub(crate) fn amount(inst: &Instances, e: &Expr, cx: Cx) -> Option<f64> {
    folded(inst, e, cx, &mut None)
}

/// `amount`, noting each `min`/`max` written in the text it starts in.
pub(crate) fn amount_choosing(
    inst: &Instances,
    e: &Expr,
    cx: Cx,
    chosen: &mut Vec<Chosen>,
) -> Option<f64> {
    folded(inst, e, cx, &mut Some(chosen))
}

/// A followed name continues in another text, with spans of its own.
fn folded(
    inst: &Instances,
    e: &Expr,
    cx: Cx,
    chosen: &mut Option<&mut Vec<Chosen>>,
) -> Option<f64> {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| folded(inst, e2, cx2, &mut None)) {
        return r;
    }
    match inst.node(e, cx) {
        Node::Lit(Literal::Num(n)) => Some(*n),
        Node::Lit(Literal::Samples(n)) => Some(n / f64::from(lattice())),
        Node::Name("pi") => Some(std::f64::consts::PI),
        Node::Name("inf") => Some(f64::INFINITY),
        Node::Name(other) => sva_formula::note::frequency(other),
        Node::Bin(op, l, r) => {
            let (a, b) = (folded(inst, l, cx, chosen)?, folded(inst, r, cx, chosen)?);
            Some(match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => a / b,
                BinOp::Mod => crate::lower::constant_modulo(a, b)?,
            })
        }
        Node::Call { name, args, span } => called(inst, (name, span), args, cx, chosen),
        // FORMAT 15.3: a ref naming one number is that number, read at bare `t`.
        Node::Read { path, arg, .. } if inst.is_now(arg, cx) => {
            let (body, held) = inst.at(path)?;
            folded(inst, body, held, &mut None)
        }
        _ => None,
    }
}

fn called(
    inst: &Instances,
    (name, at): (&str, ByteSpan),
    args: &[Arg],
    cx: Cx,
    chosen: &mut Option<&mut Vec<Chosen>>,
) -> Option<f64> {
    if name == "rand" {
        return drawn(inst, args, cx);
    }
    let mut positional = Vec::new();
    let mut named = Vec::new();
    for arg in args {
        match arg {
            Arg::Pos(x) => positional.push(folded(inst, x, cx, chosen)?),
            Arg::Named(key, x) => named.push((key.as_str(), folded(inst, x, cx, chosen)?)),
        }
    }
    let n = crate::lower::constant_call(name, &positional, &named)?;
    let won = positional.iter().position(|v| v.to_bits() == n.to_bits());
    if let (Some(held), "min" | "max", Some(won)) = (chosen.as_mut(), name, won) {
        held.push(Chosen {
            name: name.to_string(),
            at,
            operands: positional,
            chosen: won,
        });
    }
    Some(n)
}

/// A constant key is one instant of the noise, read there.
fn drawn(inst: &Instances, args: &[Arg], cx: Cx) -> Option<f64> {
    let (key, seed) = crate::lower::rand_arguments(args, |x| amount(inst, x, cx))?;
    let at = time_of(inst, key, cx)?;
    (at.scale.is_zero()).then(|| crate::lower::noise_at(seed, at.shift))
}

pub(crate) fn plain(amount: f64) -> Option<f64> {
    amount.is_finite().then_some(amount)
}
