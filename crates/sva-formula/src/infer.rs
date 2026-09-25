// Concern: decides a ClosedForm's Ty structurally, without normalizing | Non-concern: producing the spectral sum (spectral_sum/) | IO: (&ClosedForm, &Env) -> Ty or Refusal

use crate::affine::{Axis, affine, is_squared};
use crate::alg::{Alg, exponent_growth};
use crate::closed_form::{Body, Bound, ClosedForm, Part, Rational, Series, Unary, Var};
use crate::complex::C64;
use crate::env::Env;
use crate::origin::Origin;
use crate::refusal::{Code, Refusal};
use crate::series::summable;
use crate::table::class::{AtomClass, Factors, Growth};
use crate::ty::{Codomain, Held, MAX_WIDTH, Ty};

#[derive(Clone, Debug)]
struct Info {
    ty: Ty,
    alg: Alg,
}

pub fn infer(t: &ClosedForm, env: &dyn Env) -> Result<Ty, Refusal> {
    walk(&t.body, t.origin, t.var, env).map(|i| i.ty)
}

fn closed_form(alg: Alg, var: Var, width: u8, codomain: Codomain) -> Info {
    Info {
        ty: Ty {
            width,
            ..Ty::form(var, alg.in_a(), codomain)
        },
        alg,
    }
}

/// The axis a term is written on and whether its atoms still have a dual, re-read after an
/// operator has changed the algebra under it.
fn restate(info: &mut Info, var: Var) {
    info.ty.held = Held::Form(var);
    info.ty.dual = info.alg.in_a();
}

fn refuse(code: Code, origin: Origin, message: impl Into<String>) -> Refusal {
    Refusal::new(code, origin, message)
}

fn walk(f: &Body, origin: Origin, var: Var, env: &dyn Env) -> Result<Info, Refusal> {
    match f {
        Body::Const(c) => Ok(closed_form(Alg::scalar(), var, 1, codomain_of(*c))),
        Body::Line => Ok(closed_form(
            Alg::atom(AtomClass {
                factors: Factors {
                    poly: true,
                    ..Factors::default()
                },
                ..AtomClass::regular()
            }),
            var,
            1,
            match var {
                Var::T => Codomain::Real,
                Var::F => Codomain::Complex,
            },
        )),
        Body::Index(_) => Ok(closed_form(Alg::scalar(), var, 1, Codomain::Real)),
        Body::Keyed { of, .. } => Ok(closed_form(
            match crate::affine::key_moves(&of.body) {
                true => Alg::gone(),
                false => Alg::scalar(),
            },
            var,
            1,
            Codomain::Real,
        )),
        Body::Node(id) => opaque(env.node(*id), origin, var),
        Body::Param(id) => opaque(env.param(*id), origin, var),
        Body::Add(parts) => nary(parts, origin, var, env, Alg::union),
        Body::Mul(parts) => nary(parts, origin, var, env, Alg::product),
        Body::Div(num, den) => divide(num, den, origin, var, env),
        Body::Pow(base, n) => power(base, *n, var, env),
        Body::Apply(op, arg) => apply(*op, arg, var, env),
        Body::Fold(_, args) => {
            let mut info = nary(args, origin, var, env, Alg::union)?;
            if !info.alg.constant {
                info.alg = Alg::gone();
                restate(&mut info, var);
            }
            Ok(info)
        }
        Body::Delta { at, .. } => singular(at, var, env, true),
        Body::Pv(at) => singular(at, var, env, false),
        Body::Shift { of, .. } | Body::Deriv { of, .. } => walk(&of.body, of.origin, var, env),
        Body::Warp { at, of }
            if crate::affine::slide(&at.body).is_some_and(|by| by.axis() == Axis::Real) =>
        {
            walk(&of.body, of.origin, var, env)
        }
        Body::Warp { at, of } => {
            walk(&at.body, at.origin, var, env)?;
            let mut info = walk(&of.body, of.origin, var, env)?;
            info.alg = Alg::gone();
            restate(&mut info, var);
            Ok(info)
        }
        Body::Crop { of, l, r, .. } => {
            let mut info = walk(&of.body, of.origin, var, env)?;
            info.alg = info.alg.bound(l.value().is_finite(), r.value().is_finite());
            restate(&mut info, var);
            Ok(info)
        }
        Body::Join(parts) => join(parts, origin, var, env),
        Body::Channel(of, k) => channel(of, *k, origin, var, env),
        Body::Rational(r) => Ok(closed_form(rational_alg(r), var, 1, Codomain::Complex)),
        Body::Series(s) => series(s, origin, var, env),
        Body::Modal(_) => Ok(closed_form(
            Alg::atom(AtomClass {
                factors: Factors {
                    exp: true,
                    ind: true,
                    ..Factors::default()
                },
                growth: Growth::GrowsLeft,
                bounded: (true, false),
                pole_order: 0,
            }),
            var,
            1,
            Codomain::Real,
        )),
    }
}

fn codomain_of(c: C64) -> Codomain {
    if c.is_real() {
        Codomain::Real
    } else {
        Codomain::Complex
    }
}

/// A node or a parameter answers with its own type; this crate learns nothing about the
/// atoms behind it, only whether they were already in A.
fn opaque(ty: Ty, origin: Origin, var: Var) -> Result<Info, Refusal> {
    if ty.is_closed_form() && !ty.has_dual() && ty.held != Held::Form(var) {
        return Err(refuse(
            Code::DomainMismatch,
            origin,
            format!(
                "this reads {}, and the closed form is written in {}",
                other(var).as_str(),
                var.as_str()
            ),
        ));
    }
    let alg = match ty.has_dual() {
        true => Alg::atom(AtomClass::regular()),
        false => Alg::gone(),
    };
    Ok(Info {
        ty: ty.read_on(var),
        alg,
    })
}

fn other(var: Var) -> Var {
    match var {
        Var::T => Var::F,
        Var::F => Var::T,
    }
}

fn nary(
    parts: &[Part],
    origin: Origin,
    var: Var,
    env: &dyn Env,
    mut combine: impl FnMut(Alg, Alg) -> Alg,
) -> Result<Info, Refusal> {
    let mut acc: Option<Info> = None;
    for p in parts {
        let next = walk(&p.body, p.origin, var, env)?;
        acc = Some(match acc {
            None => next,
            Some(a) => {
                let ty =
                    a.ty.meet(next.ty)
                        .map_err(|m| Refusal::of_mismatch(m, origin, a.ty, next.ty))?;
                let alg = combine(a.alg, next.alg);
                let mut info = Info { ty, alg };
                if ty.is_closed_form() {
                    restate(&mut info, var);
                }
                info
            }
        });
    }
    Ok(acc.unwrap_or_else(|| closed_form(Alg::scalar(), var, 1, Codomain::Real)))
}

fn divide(
    num: &Part,
    den: &Part,
    origin: Origin,
    var: Var,
    env: &dyn Env,
) -> Result<Info, Refusal> {
    let a = walk(&num.body, num.origin, var, env)?;
    let b = walk(&den.body, den.origin, var, env)?;
    let ty =
        a.ty.meet(b.ty)
            .map_err(|m| Refusal::of_mismatch(m, origin, a.ty, b.ty))?;
    let alg = if b.alg.constant {
        a.alg.product(b.alg)
    } else {
        Alg::gone()
    };
    let mut info = Info { ty, alg };
    if ty.is_closed_form() {
        restate(&mut info, var);
    }
    Ok(info)
}

fn power(base: &Part, n: i32, var: Var, env: &dyn Env) -> Result<Info, Refusal> {
    let inner = walk(&base.body, base.origin, var, env)?;
    if !inner.ty.is_closed_form() {
        return Ok(inner);
    }
    let alg = match n {
        0 => Alg::scalar(),
        n if n < 0 && inner.alg.constant => Alg {
            escaped: inner.alg.escaped,
            ..Alg::scalar()
        },
        n if n > 0 => (1..n).fold(inner.alg.clone(), |acc, _| acc.product(inner.alg.clone())),
        n => Alg {
            classes: vec![AtomClass {
                factors: Factors {
                    pole: true,
                    ..Factors::default()
                },
                pole_order: u16::try_from(n.unsigned_abs()).unwrap_or(u16::MAX),
                ..AtomClass::regular()
            }],
            constant: inner.alg.constant,
            escaped: inner.alg.escaped,
        },
    };
    Ok(closed_form(alg, var, inner.ty.width, inner.ty.codomain))
}

fn apply(op: Unary, arg: &Part, var: Var, env: &dyn Env) -> Result<Info, Refusal> {
    let inner = walk(&arg.body, arg.origin, var, env)?;
    if !inner.ty.is_closed_form() || inner.alg.constant {
        return Ok(inner);
    }
    let alg = match (op.is_closed(), affine(&arg.body)) {
        (true, Some((slope, _))) => Alg::atom(AtomClass {
            factors: Factors {
                exp: true,
                ..Factors::default()
            },
            growth: exponent_growth(op, slope),
            ..AtomClass::regular()
        }),
        (true, None) if op == Unary::Exp && is_squared(&arg.body) => Alg::atom(AtomClass {
            factors: Factors {
                gauss: true,
                exp: true,
                ..Factors::default()
            },
            ..AtomClass::regular()
        }),
        _ => Alg::gone(),
    };
    Ok(closed_form(alg, var, inner.ty.width, inner.ty.codomain))
}

fn singular(at: &Part, var: Var, env: &dyn Env, is_delta: bool) -> Result<Info, Refusal> {
    walk(&at.body, at.origin, var, env)?;
    let placed = affine(&at.body)
        .is_some_and(|(a, b)| a.axis() == Axis::Real && b.axis() == Axis::Real && !a.is_zero());
    if !placed {
        return Err(refuse(
            Code::NonAffineSingular,
            at.origin,
            format!(
                "{} needs an argument affine in {}",
                if is_delta { "delta" } else { "pv" },
                var.as_str()
            ),
        ));
    }
    let factors = if is_delta {
        Factors {
            delta: true,
            ..Factors::default()
        }
    } else {
        Factors {
            pole: true,
            pv: true,
            ..Factors::default()
        }
    };
    let alg = Alg::atom(AtomClass {
        factors,
        pole_order: u16::from(!is_delta),
        ..AtomClass::regular()
    });
    let codomain = if is_delta {
        Codomain::Real
    } else {
        Codomain::Complex
    };
    Ok(closed_form(alg, var, 1, codomain))
}

fn join(parts: &[Part], origin: Origin, var: Var, env: &dyn Env) -> Result<Info, Refusal> {
    let mut width = 0u32;
    let mut alg: Option<Alg> = None;
    let mut codomain = Codomain::Real;
    for p in parts {
        let info = walk(&p.body, p.origin, var, env)?;
        width += u32::from(info.ty.width);
        codomain = codomain.join(info.ty.codomain);
        alg = Some(match alg {
            None => info.alg,
            Some(a) => a.union(info.alg),
        });
    }
    let Ok(width) = u8::try_from(width) else {
        return Err(width_refusal(origin, width));
    };
    if width > MAX_WIDTH {
        return Err(width_refusal(origin, u32::from(width)));
    }
    Ok(closed_form(
        alg.unwrap_or_else(Alg::scalar),
        var,
        width,
        codomain,
    ))
}

fn width_refusal(origin: Origin, width: u32) -> Refusal {
    refuse(
        Code::WidthMismatch,
        origin,
        format!("{width} components join; a value carries at most {MAX_WIDTH}"),
    )
}

fn channel(of: &Part, k: u8, origin: Origin, var: Var, env: &dyn Env) -> Result<Info, Refusal> {
    let info = walk(&of.body, of.origin, var, env)?;
    if k >= info.ty.width {
        return Err(refuse(
            Code::WidthMismatch,
            origin,
            format!("component {k} of a value {} components wide", info.ty.width),
        ));
    }
    Ok(Info {
        ty: Ty {
            width: 1,
            ..info.ty
        },
        alg: info.alg,
    })
}

fn rational_alg(r: &Rational) -> Alg {
    Alg::atom(AtomClass {
        factors: Factors {
            pole: true,
            ..Factors::default()
        },
        pole_order: u16::try_from(r.poles.len()).unwrap_or(u16::MAX),
        ..AtomClass::regular()
    })
}

fn series(s: &Series, origin: Origin, var: Var, env: &dyn Env) -> Result<Info, Refusal> {
    let term = walk(&s.term.body, s.term.origin, var, env)?;
    if matches!(s.hi, Bound::Infinite) && !summable(s, env) {
        return Err(refuse(
            Code::SeriesNotSummable,
            origin,
            "the series does not converge: its coefficients are not polynomially bounded in \
             the index",
        ));
    }
    Ok(term)
}
