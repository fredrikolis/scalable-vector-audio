// Concern: classifies a self-reference, and folds a read's time or a number to its exact value | Non-concern: running a loop (sva-samples), lowering (lower/) | IO: (body, Cx) -> SelfKind, Affine

use sva_ast::{Address, Arg, BinOp, ByteSpan, Expr, Literal};
use sva_formula::closed_form::{map_children, read_at};
use sva_formula::{Body, C64, IndexId, Part, Series, Var};

use crate::arguments::Chosen;
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::{Cx, Instances, Node};
use crate::time::{Affine, Q};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SelfKind {
    Series {
        gain: C64,
        delay: f64,
    },
    /// A sequence stepped on the grid its reader induces; `why` names the construct that
    /// makes it one.
    Discrete {
        why: String,
    },
    Refuse(Box<EngineError>),
}

/// Decided before any rate is. One linear `self` a constant delay back at `abs(g) < 1` over a
/// closed form is continuous, its series. Any other loop is discrete and names why: a call or
/// product over `self`, a moving or second delay, or `discrete`, samples already in its body.
pub(crate) fn classify(
    inst: &Instances,
    e: &Expr,
    cx: Cx,
    at: &str,
    discrete: Option<String>,
) -> SelfKind {
    let (taps, apart) = match read(inst, e, cx, C64::ONE) {
        Reading::Refused(tap) => {
            return SelfKind::Refuse(Box::new(
                tap_refusal(tap, Located::at(at, None)).expect("a refused tap names its reason"),
            ));
        }
        Reading::Nonlinear(why) => return SelfKind::Discrete { why },
        Reading::Free => (Vec::new(), None),
        Reading::Linear { taps, apart } => (taps, apart),
    };
    match (&discrete, &apart, taps.as_slice()) {
        (None, None, [(g, Tap::Back(delay))]) if g.abs() < 1.0 => {
            return SelfKind::Series {
                gain: *g,
                delay: delay.to_f64(),
            };
        }
        (None, None, [(g, Tap::Back(_))]) => return SelfKind::Refuse(Box::new(unbounded(*g, at))),
        (Some(_), None, [(g, Tap::Back(_))]) if g.abs() > 1.0 => {
            return SelfKind::Refuse(Box::new(unbounded(*g, at)));
        }
        _ => {}
    }
    let moving = taps
        .iter()
        .any(|(_, tap)| *tap == Tap::Moving)
        .then(|| "a delay that moves".to_string());
    let second = (taps.len() > 1).then(|| "a second delay of `self`".to_string());
    let why = moving
        .or(apart)
        .or(discrete)
        .or(second)
        .expect("a loop that is no series holds what makes it discrete");
    SelfKind::Discrete { why }
}

/// What one `self(...)` call site reads: a constant delay back, a time that moves, whole
/// samples at a delay that moves, or the reason it reads nothing already written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Tap {
    Back(Q),
    Moving,
    Indexed,
    Zero,
    Forward,
}

/// The one reading of a self-reference's time, so classification and lowering cannot drift.
pub(crate) fn tap_of(inst: &Instances, arg: &Expr, address: Address, cx: Cx) -> Tap {
    if address == Address::Index {
        return indexed_tap(inst, arg, cx);
    }
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

/// A loop steps at the rate in use, so its index reads one delay back or a delay that moves.
/// An index its lowering refuses reads as one that moves until the lowering says so.
fn indexed_tap(inst: &Instances, arg: &Expr, cx: Cx) -> Tap {
    let map = crate::index::read(inst, arg, cx).and_then(|ix| ix.map(cx.grid));
    let Some(map) = map.filter(|m| m.a == m.d) else {
        return Tap::Indexed;
    };
    match (map.least(), map.lead()) {
        (_, most) if most > 0 => Tap::Forward,
        (_, 0) => Tap::Zero,
        (least, most) if least == most => cx
            .grid
            .steps(Q::int(-least))
            .map_or(Tap::Indexed, Tap::Back),
        _ => Tap::Indexed,
    }
}

pub(crate) fn tap_refusal(tap: Tap, at: Located) -> Option<EngineError> {
    let (code, message, help) = match tap {
        Tap::Back(_) | Tap::Moving | Tap::Indexed => return None,
        Tap::Zero => (
            "samples.zero_delay_loop",
            "a loop reaches no sample it has already written.",
            "write self[idx(t) - 1] for a one-step loop",
        ),
        Tap::Forward => (
            "engine.forward_self_read",
            "a loop reads its own output before it is written.",
            "read an earlier sample, as in self[idx(t) - 1]",
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
        help: "write a gain under 1, or step a running sum as a discrete loop, as in \
               self[idx(t) - 1]"
            .to_string(),
    })
}

/// What one subterm gives: nothing, scaled reads of `self`, or a construct no series spells,
/// named. A component taken or joined keeps its taps, but `apart` names the call that took
/// them out of one series.
enum Reading {
    Free,
    Linear {
        taps: Vec<(C64, Tap)>,
        apart: Option<String>,
    },
    Refused(Tap),
    Nonlinear(String),
}

fn read(inst: &Instances, e: &Expr, cx: Cx, gain: C64) -> Reading {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| read(inst, e2, cx2, gain)) {
        return r;
    }
    let product = || Reading::Nonlinear("`self` times a factor that moves".to_string());
    match inst.node(e, cx) {
        Node::Own { arg, address, .. } => match tap_of(inst, arg, address, cx) {
            tap @ (Tap::Back(_) | Tap::Moving | Tap::Indexed) => Reading::Linear {
                taps: vec![(gain, tap)],
                apart: None,
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
                None => product(),
            },
            (true, false) => match constant(inst, r, cx) {
                Some(k) => read(inst, l, cx, gain * k),
                None => product(),
            },
            (false, false) => Reading::Free,
            (true, true) => product(),
        },
        Node::Bin(BinOp::Div, l, r) => match (holds(inst, l, cx), holds(inst, r, cx)) {
            (true, false) => match constant(inst, r, cx) {
                Some(k) if !k.is_zero() => read(inst, l, cx, gain / k),
                _ => Reading::Nonlinear("`self` over a divisor that moves".to_string()),
            },
            (false, false) => Reading::Free,
            _ => Reading::Nonlinear("a division by `self`".to_string()),
        },
        Node::Call { name, args, .. } if name == crate::vocabulary::CHANNEL => match args {
            [Arg::Pos(x), Arg::Pos(k)] if !holds(inst, k, cx) => {
                opaque(read(inst, x, cx, gain), name)
            }
            _ => construct(name),
        },
        Node::Call { name, args, .. } if name == crate::vocabulary::JOIN => args
            .iter()
            .map(|a| match a {
                Arg::Pos(x) => opaque(read(inst, x, cx, gain), name),
                Arg::Named(..) => construct(name),
            })
            .fold(Reading::Free, join),
        other => match (holds_in(inst, &other, cx), &other) {
            (false, _) => Reading::Free,
            (true, Node::Call { name, .. }) => construct(name),
            (true, Node::Read { path, .. }) => {
                Reading::Nonlinear(format!("`@{path}` read at a time `self` moves"))
            }
            (true, _) => Reading::Nonlinear("`%` over `self`".to_string()),
        },
    }
}

/// A call over the loop's own past: a filter, which holds state of its own, or any other,
/// which no series expands.
fn construct(name: &str) -> Reading {
    Reading::Nonlinear(match sva_formula::filter::Shape::from_name(name) {
        Some(_) => format!("the filter `{name}(...)`"),
        None => format!("`{name}(...)` over `self`"),
    })
}

fn opaque(r: Reading, by: &str) -> Reading {
    match r {
        Reading::Linear { taps, apart } => Reading::Linear {
            taps,
            apart: apart.or_else(|| Some(format!("`{by}(...)` over `self`"))),
        },
        other => other,
    }
}

fn join(a: Reading, b: Reading) -> Reading {
    match (a, b) {
        (Reading::Refused(tap), _) | (_, Reading::Refused(tap)) => Reading::Refused(tap),
        (Reading::Nonlinear(why), _) | (_, Reading::Nonlinear(why)) => Reading::Nonlinear(why),
        (Reading::Free, other) | (other, Reading::Free) => other,
        (
            Reading::Linear {
                taps: mut held,
                apart: a1,
            },
            Reading::Linear {
                taps: more,
                apart: a2,
            },
        ) => {
            for (g, tap) in more {
                match held
                    .iter_mut()
                    .find(|(_, t)| *t == tap && matches!(tap, Tap::Back(_)))
                {
                    Some((sum, _)) => *sum = *sum + g,
                    None => held.push((g, tap)),
                }
            }
            Reading::Linear {
                taps: held,
                apart: a1.or(a2),
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
        Node::Lit(Literal::Samples(n)) => Some(Term::Number(cx.grid.steps(Q::decimal(*n)?)?)),
        _ => Q::decimal(plain(amount(inst, e, cx)?)?).map(Term::Number),
    }
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
        Node::Lit(Literal::Samples(n)) => Some(cx.grid.steps_f64(*n)),
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
        Node::Read {
            path,
            arg,
            address: Address::Time,
            ..
        } if inst.is_now(arg, cx) => {
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
    at.scale
        .is_zero()
        .then(|| crate::lower::noise_at(seed, at.shift, inst.rate()))
}

pub(crate) fn plain(amount: f64) -> Option<f64> {
    amount.is_finite().then_some(amount)
}
