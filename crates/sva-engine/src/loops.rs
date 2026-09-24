// Concern: classifies a self-reference, and folds the shift one reads at to a constant | Non-concern: running either kind (sva-samples), lowering the rest (lower/) | IO: (body, Cx) -> SelfKind, Shift

use sva_ast::{Arg, BinOp, ByteSpan, Expr, Literal};
use sva_formula::closed_form::map_children;
use sva_formula::{Body, C64, IndexId, Part, Series, Var};

use crate::arguments::Chosen;
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::{Cx, Instances, Node};

/// A sampled loop's step count is only known once an observation names a rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Delay {
    Steps(u32),
    Secs(f64),
    /// A delay written as a closed form of `t`, which only the grid can follow.
    Varying,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SelfKind {
    Series { gain: C64, delay: f64 },
    Sampled,
    Refuse(Box<EngineError>),
}

/// Linear in `self`, a constant delay in seconds and `abs(g) < 1` is a series, and the same
/// loop at unit gain or above settles nowhere. A delay in `sp` is row three of FORMAT 11 and
/// runs on the grid at unit gain, a running sum being a value; above it, nothing settles.
pub(crate) fn classify(inst: &Instances, e: &Expr, cx: Cx, at: &str) -> SelfKind {
    match read(inst, e, cx, C64::ONE) {
        Reading::Refused(tap) => SelfKind::Refuse(Box::new(
            tap_refusal(tap, Located::at(at, None)).expect("a refused tap names its reason"),
        )),
        Reading::Free | Reading::Nonlinear => SelfKind::Sampled,
        Reading::Linear {
            gain,
            delay: Delay::Steps(_),
        } if gain.abs() > 1.0 => SelfKind::Refuse(Box::new(unbounded(gain, at))),
        Reading::Linear {
            delay: Delay::Steps(_) | Delay::Varying,
            ..
        } => SelfKind::Sampled,
        Reading::Linear {
            gain,
            delay: Delay::Secs(secs),
        } => match gain.abs() < 1.0 {
            true => SelfKind::Series { gain, delay: secs },
            false => SelfKind::Refuse(Box::new(unbounded(gain, at))),
        },
    }
}

/// What one `self(...)` call site reads: a usable delay, or the reason it is not one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Tap {
    At(Delay),
    Zero,
    Forward,
    Fractional,
}

/// The one reading of a self-reference's time, so classification and lowering cannot drift.
pub(crate) fn tap_of(inst: &Instances, arg: &Expr, cx: Cx) -> Tap {
    match shift_of(inst, arg, cx) {
        None => Tap::At(Delay::Varying),
        Some(Shift::Now) => Tap::Zero,
        Some(Shift::Secs(secs)) if secs > 0.0 => Tap::At(Delay::Secs(secs)),
        Some(Shift::Steps(steps)) if steps > 0.0 && steps.fract() == 0.0 => {
            Tap::At(Delay::Steps(steps as u32))
        }
        Some(Shift::Steps(steps)) if steps > 0.0 => Tap::Fractional,
        Some(_) => Tap::Forward,
    }
}

pub(crate) fn tap_refusal(tap: Tap, at: Located) -> Option<EngineError> {
    let (code, message, help) = match tap {
        Tap::At(_) => return None,
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
        Tap::Fractional => (
            "ref.fractional_shift_on_samples",
            "a read on the grid moves by whole samples.",
            "write a whole number of sp, or the delay in seconds",
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

/// What one subterm gives: nothing, one scaled delayed read, or a shape no series expands.
enum Reading {
    Free,
    Linear { gain: C64, delay: Delay },
    Refused(Tap),
    Nonlinear,
}

fn read(inst: &Instances, e: &Expr, cx: Cx, gain: C64) -> Reading {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| read(inst, e2, cx2, gain)) {
        return r;
    }
    match inst.node(e, cx) {
        Node::Own { arg, .. } => match tap_of(inst, arg, cx) {
            Tap::At(delay) => Reading::Linear { gain, delay },
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
        other => match holds_in(inst, &other, cx) {
            true => Reading::Nonlinear,
            false => Reading::Free,
        },
    }
}

fn join(a: Reading, b: Reading) -> Reading {
    match (a, b) {
        (Reading::Refused(tap), _) | (_, Reading::Refused(tap)) => Reading::Refused(tap),
        (Reading::Nonlinear, _) | (_, Reading::Nonlinear) => Reading::Nonlinear,
        (Reading::Free, other) | (other, Reading::Free) => other,
        (
            Reading::Linear {
                gain: g1,
                delay: d1,
            },
            Reading::Linear {
                gain: g2,
                delay: d2,
            },
        ) if d1 == d2 => Reading::Linear {
            gain: g1 + g2,
            delay: d1,
        },
        _ => Reading::Nonlinear,
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

/// `t -> t - k*d` at every occurrence of the free variable.
fn moved(f: &Body, index: IndexId, delay: f64) -> Body {
    match f {
        Body::Line => Body::Add(vec![
            Part::bare(Body::Line),
            Part::bare(Body::Mul(vec![
                Part::bare(Body::Const(C64::real(-delay))),
                Part::bare(Body::Index(index)),
            ])),
        ]),
        other => map_children(other, |p| Part::new(p.origin, moved(&p.body, index, delay))),
    }
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

/// A read at the sample being written, at a constant delay in seconds, or at whole steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shift {
    Now,
    Secs(f64),
    Steps(f64),
}

/// `t` offset by a constant is the whole grammar of a read's time; the constant may be
/// written in seconds or in grid steps, but never in both.
pub fn shift_of(inst: &Instances, e: &Expr, cx: Cx) -> Option<Shift> {
    let (time, secs, steps) = walk(inst, e, cx, 1.0)?;
    if !time {
        return None;
    }
    match (secs, steps) {
        (0.0, 0.0) => Some(Shift::Now),
        (secs, 0.0) => Some(Shift::Secs(-secs)),
        (0.0, steps) => Some(Shift::Steps(-steps)),
        _ => None,
    }
}

/// `(reads t, seconds offset, grid-step offset)`, signed as written.
fn walk(inst: &Instances, e: &Expr, cx: Cx, sign: f64) -> Option<(bool, f64, f64)> {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| walk(inst, e2, cx2, sign)) {
        return r;
    }
    match inst.node(e, cx) {
        Node::Name("t") => Some((true, 0.0, 0.0)),
        Node::Bin(op @ (BinOp::Add | BinOp::Sub), l, r) => {
            let (lt, ls, lg) = walk(inst, l, cx, sign)?;
            let flip = if op == BinOp::Sub { -sign } else { sign };
            let (rt, rs, rg) = walk(inst, r, cx, flip)?;
            Some((lt || rt, ls + rs, lg + rg))
        }
        _ => {
            let (secs, steps) = amount(inst, e, cx)?;
            Some((false, sign * secs, sign * steps))
        }
    }
}

/// A written duration over every operator FORMAT 3.3 folds, seconds and grid steps kept
/// apart. What this drops is defaulted, never refused.
pub(crate) fn amount(inst: &Instances, e: &Expr, cx: Cx) -> Option<(f64, f64)> {
    folded(inst, e, cx, &mut None)
}

/// `amount`, noting each `min`/`max` written in the text it starts in.
pub(crate) fn amount_choosing(
    inst: &Instances,
    e: &Expr,
    cx: Cx,
    chosen: &mut Vec<Chosen>,
) -> Option<(f64, f64)> {
    folded(inst, e, cx, &mut Some(chosen))
}

/// A followed name continues in another text, with spans of its own.
fn folded(
    inst: &Instances,
    e: &Expr,
    cx: Cx,
    chosen: &mut Option<&mut Vec<Chosen>>,
) -> Option<(f64, f64)> {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| folded(inst, e2, cx2, &mut None)) {
        return r;
    }
    match inst.node(e, cx) {
        Node::Lit(Literal::Num(n)) => Some((*n, 0.0)),
        Node::Lit(Literal::Samples(n)) => Some((0.0, *n)),
        Node::Name("pi") => Some((std::f64::consts::PI, 0.0)),
        Node::Name(other) => sva_formula::note::frequency(other).map(|hz| (hz, 0.0)),
        Node::Bin(op, l, r) => {
            let (a, b) = (folded(inst, l, cx, chosen)?, folded(inst, r, cx, chosen)?);
            Some(match op {
                BinOp::Add => (a.0 + b.0, a.1 + b.1),
                BinOp::Sub => (a.0 - b.0, a.1 - b.1),
                BinOp::Mul => scaled(a, b)?,
                BinOp::Div => {
                    let by = plain(b)?;
                    (a.0 / by, a.1 / by)
                }
                BinOp::Mod => (crate::lower::constant_modulo(plain(a)?, plain(b)?)?, 0.0),
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

/// One side of a product carries the unit and the other is the plain number scaling it.
fn scaled(a: (f64, f64), b: (f64, f64)) -> Option<(f64, f64)> {
    match (plain(a), plain(b)) {
        (Some(k), _) => Some((k * b.0, k * b.1)),
        (_, Some(k)) => Some((k * a.0, k * a.1)),
        _ => None,
    }
}

fn called(
    inst: &Instances,
    (name, at): (&str, ByteSpan),
    args: &[Arg],
    cx: Cx,
    chosen: &mut Option<&mut Vec<Chosen>>,
) -> Option<(f64, f64)> {
    let mut positional = Vec::new();
    let mut named = Vec::new();
    for arg in args {
        match arg {
            Arg::Pos(x) => positional.push(plain(folded(inst, x, cx, chosen)?)?),
            Arg::Named(key, x) => {
                named.push((key.as_str(), plain(folded(inst, x, cx, chosen)?)?));
            }
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
    Some((n, 0.0))
}

pub(crate) fn plain(amount: (f64, f64)) -> Option<f64> {
    (amount.1 == 0.0 && amount.0.is_finite()).then_some(amount.0)
}
