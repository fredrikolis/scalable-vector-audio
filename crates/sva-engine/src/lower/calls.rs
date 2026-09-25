// Concern: the formula each written call lowers to | Non-concern: the recursion over an Expr (mod.rs) | IO: (name, args) -> a Piece

use sva_ast::{Arg, ByteSpan, Expr};
use sva_formula::filter::Shape;
use sva_formula::{Body, C64, Codomain, Edge, Fold, Held, Origin, Part, Ty, Unary, Var, hash};

use crate::arguments::{Argument, Called, Chosen};
use crate::cast::Cast;
use crate::error::EngineError;
use crate::instantiate::Cx;
use crate::lower::{Lowering, Piece};
use crate::typing::Value;
use crate::vocabulary::SERIES;

impl<'g> Lowering<'_, 'g> {
    pub(super) fn call(
        &mut self,
        name: &str,
        args: &[Arg],
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        if name == SERIES {
            return self.sum(args, span, cx, var);
        }
        let written = args.iter().filter(|a| matches!(a, Arg::Pos(_))).count();
        let keys: Vec<String> = args
            .iter()
            .filter_map(|a| match a {
                Arg::Named(k, _) => Some(k.clone()),
                Arg::Pos(_) => None,
            })
            .collect();
        crate::overload::check_arity(name, written, &keys)
            .map_err(|m| self.refuse(name, &m, Some(span)))?;
        let mut chosen = Vec::new();
        let named = self.named_values(name, args, span, cx, &mut chosen)?;
        let view: Vec<(&str, f64)> = named.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        let solved = crate::overload::FINITE_DIFFERENCE.contains(&name);
        if solved || super::physics::MODAL.contains(&name) {
            let Some(numbers) = positional_values(self, args, cx, &mut chosen) else {
                return Err(EngineError::BadArity(name.to_string()));
            };
            if !solved {
                self.note_call(name, span, positional_named(name, &numbers, &named), chosen);
                return self.modal(name, &numbers, &view);
            }
            return self.solver(name, &numbers, &view, span, chosen, var);
        }
        if name == "noise" {
            let Some(numbers) = positional_values(self, args, cx, &mut chosen) else {
                return Err(EngineError::BadArity(name.to_string()));
            };
            let [seed, ..] = numbers.as_slice() else {
                return Err(EngineError::BadArity(name.to_string()));
            };
            self.note_call(name, span, positional_named(name, &numbers, &named), chosen);
            return Ok(Piece::ClosedForm(Body::Series(Box::new(
                sva_formula::noise(
                    *seed as u64,
                    named_or(&view, "period", 1.0),
                    named_or(&view, "color", 0.0),
                ),
            ))));
        }
        self.note_call(name, span, written_named(&named), chosen);
        if let Some(cast) = Cast::from_name(name, &view) {
            return self.cast(cast, args, span, cx, var);
        }
        if let Some(shape) = Shape::from_name(name) {
            return self.filter(shape, args, span, cx, var);
        }
        let positional: Vec<&Expr> = args
            .iter()
            .filter_map(|a| match a {
                Arg::Pos(x) => Some(x),
                Arg::Named(..) => None,
            })
            .collect();
        if name == "rand" {
            return self.drawn(&positional, &view, cx, span, var);
        }
        if let Some(piece) = self.wave(name, &positional, &view, cx, var)? {
            return Ok(piece);
        }
        let mut pieces = Vec::with_capacity(positional.len());
        for x in &positional {
            pieces.push(self.walk(x, cx, var)?);
        }
        if pieces.iter().any(|p| matches!(p, Piece::Value(_))) {
            return match (name, view.iter().find(|(k, _)| *k == "drive")) {
                ("sat", Some((_, drive))) => self.driven(pieces, *drive, span, var),
                ("crop", _) => self.sampled_crop(pieces, &view, span, var),
                _ => self.operation(name, pieces, Some(span), var),
            };
        }
        let bodies: Vec<Body> = pieces
            .into_iter()
            .map(|p| match p {
                Piece::ClosedForm(f) => f,
                Piece::Value(_) => unreachable!("every piece here stayed inside the closed form"),
            })
            .collect();
        self.image(name, bodies, &positional, &view, span, var)
    }

    /// The atoms a written name lowers to, once every operand is inside the same closed form.
    fn image(
        &mut self,
        name: &str,
        bodies: Vec<Body>,
        written: &[&Expr],
        named: &[(&str, f64)],
        span: ByteSpan,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let read = |key: &str, fallback: f64| named_or(named, key, fallback);
        let arity = |want: usize| -> Result<(), EngineError> {
            match bodies.len() == want {
                true => Ok(()),
                false => Err(EngineError::BadArity(name.to_string())),
            }
        };
        if numeric(name) {
            let origin = self.typing.mark(self.here(Some(span)));
            return match arithmetic(name, &bodies, named, var, origin) {
                Some(body) => Ok(Piece::ClosedForm(body)),
                None if name == "pow" && bodies.len() == 2 => {
                    Err(self.non_integer_power(written, span))
                }
                None => Err(EngineError::BadArity(name.to_string())),
            };
        }
        match name {
            "crop" => {
                arity(3)?;
                let (rise, fall) = shoulders_of(named);
                let (l, r) = self.cropped(&bodies[1], &bodies[2], (rise, fall), var, span)?;
                let of = self.part(bodies[0].clone(), Some(span));
                Ok(Piece::ClosedForm(Body::Crop {
                    of,
                    l,
                    r,
                    rise,
                    fall,
                }))
            }
            "delta" => {
                arity(1)?;
                let at = self.part(bodies[0].clone(), Some(span));
                Ok(Piece::ClosedForm(Body::Delta {
                    at,
                    order: read("k", 0.0) as u16,
                }))
            }
            "pv" => {
                arity(1)?;
                let at = self.part(bodies[0].clone(), Some(span));
                Ok(Piece::ClosedForm(Body::Pv(at)))
            }
            "join" => {
                let parts = self.parts(bodies, span);
                Ok(Piece::ClosedForm(Body::Join(parts)))
            }
            "ch" => {
                arity(2)?;
                let Some(index) = super::constant_value(&bodies[1], var) else {
                    return Err(EngineError::BadArity(name.to_string()));
                };
                let of = self.part(bodies[0].clone(), Some(span));
                Ok(Piece::ClosedForm(Body::Channel(of, index as u8)))
            }
            other => Err(EngineError::UnknownBuiltin(other.to_string())),
        }
    }

    /// The product `arithmetic` writes, so the drive reaches the renderer and the identity.
    fn driven(
        &mut self,
        mut pieces: Vec<Piece>,
        drive: f64,
        span: ByteSpan,
        var: Var,
    ) -> Result<Piece, EngineError> {
        pieces.push(Piece::ClosedForm(Body::Const(C64::real(drive))));
        let product = self.operation("*", pieces, Some(span), var)?;
        self.operation("sat", vec![product], Some(span), var)
    }

    /// A closed form's window: its shoulders, refused alike, ride along as two numbers.
    fn sampled_crop(
        &mut self,
        mut pieces: Vec<Piece>,
        named: &[(&str, f64)],
        span: ByteSpan,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let (rise, fall) = shoulders_of(named);
        if let [_, Piece::ClosedForm(l), Piece::ClosedForm(r)] = pieces.as_slice() {
            self.cropped(l, r, (rise, fall), var, span)?;
        }
        if rise > 0.0 || fall > 0.0 {
            for shoulder in [rise, fall] {
                pieces.push(Piece::ClosedForm(Body::Const(C64::real(shoulder))));
            }
        }
        self.operation("crop", pieces, Some(span), var)
    }

    fn non_integer_power(&self, written: &[&Expr], span: ByteSpan) -> EngineError {
        let [_, exponent] = written else {
            unreachable!("pow checked its arity before it lowered either operand")
        };
        self.refused_at(
            "type.non_integer_power",
            format!(
                "`pow` raises a signal to `{}`, which names no polynomial power.",
                sva_ast::render_expr(exponent)
            ),
            "write a whole exponent within 65535, or a positive constant base, which raises \
             as exp(x*ln(base))",
            Some(span),
        )
    }

    /// A window's bounds are two numbers, and a constant expression is one: every ref
    /// naming a number folds first, so `@variables/bar*3` is the second a bar names. Only a
    /// bound that moves with the free variable has no number to be.
    fn window(
        &mut self,
        l: &Body,
        r: &Body,
        var: Var,
        name: &str,
        span: ByteSpan,
    ) -> Result<(Edge, Edge), EngineError> {
        Ok((
            self.edge(l, var, name, "start", span)?,
            self.edge(r, var, name, "end", span)?,
        ))
    }

    /// A crop's window and shoulders, for either representation of its operand.
    fn cropped(
        &mut self,
        l: &Body,
        r: &Body,
        (rise, fall): (f64, f64),
        var: Var,
        span: ByteSpan,
    ) -> Result<(Edge, Edge), EngineError> {
        let (l, r) = self.window(l, r, var, "crop", span)?;
        self.shoulders(rise, fall, r.value() - l.value(), span)?;
        Ok((l, r))
    }

    /// A shoulder opens inside the window it belongs to. Zero is a hard edge, a negative one
    /// opens before the window starts, and two that overlap add energy the term never had.
    fn shoulders(&self, rise: f64, fall: f64, span: f64, at: ByteSpan) -> Result<(), EngineError> {
        let refuse = |what: String| {
            Err(self.refused_at(
                "engine.bad_crop_shoulder",
                what,
                "write a rise and a fall at or above zero that together fit the window",
                Some(at),
            ))
        };
        if rise < 0.0 || fall < 0.0 {
            return refuse(format!(
                "`crop` reads rise={rise} and fall={fall}; a shoulder \
                 opens forward in time."
            ));
        }
        if rise + fall > span {
            return refuse(format!(
                "`crop`'s rise={rise} and fall={fall} are {} together, longer than the {span} \
                 the window itself runs.",
                rise + fall
            ));
        }
        Ok(())
    }

    fn edge(
        &self,
        body: &Body,
        var: Var,
        name: &str,
        which: &str,
        span: ByteSpan,
    ) -> Result<Edge, EngineError> {
        let folded = crate::refs::fold_constants(self.typing, body);
        if super::never(&folded) {
            return Ok(Edge::PosInf);
        }
        match super::constant_value(&folded, var) {
            Some(x) => Ok(Edge::at(x)),
            None => Err(self.refused_at(
                "engine.non_constant_argument",
                format!("`{name}` reads `{which}` as a number, and this one moves."),
                "write a constant there; a bound that moves with t names no window",
                Some(span),
            )),
        }
    }

    fn parts(&mut self, bodies: Vec<Body>, span: ByteSpan) -> Vec<Part> {
        bodies
            .into_iter()
            .map(|b| self.part(b, Some(span)))
            .collect()
    }

    /// Every named number a call reads. One the vocabulary lets move is its builtin's to route
    /// when it folds to none; any other that folds to none is refused, never defaulted.
    fn named_values(
        &self,
        name: &str,
        args: &[Arg],
        span: ByteSpan,
        cx: Cx,
        chosen: &mut Vec<Chosen>,
    ) -> Result<Vec<(String, f64)>, EngineError> {
        let mut out = Vec::new();
        for arg in args {
            let Arg::Named(key, value) = arg else {
                continue;
            };
            match self.chosen_value(value, cx, chosen) {
                Some(v) => out.push((key.clone(), v)),
                None if crate::vocabulary::named_may_move(name, key) => {}
                None => {
                    return Err(self.refused_at(
                        "engine.non_constant_argument",
                        format!(
                            "`{name}` reads `{key}={}` as one number, and it names none.",
                            sva_ast::render_expr(value)
                        ),
                        "write a constant there: a number, a unit, or a ref naming one",
                        Some(span),
                    ));
                }
            }
        }
        Ok(out)
    }

    fn chosen_value(&self, e: &Expr, cx: Cx, chosen: &mut Vec<Chosen>) -> Option<f64> {
        crate::loops::plain(crate::loops::amount_choosing(self.inst, e, cx, chosen)?)
    }

    /// The numbers a call was lowered with, where it was written, and what each constant
    /// `min`/`max` inside them chose.
    fn note_call(
        &mut self,
        name: &str,
        at: ByteSpan,
        arguments: Vec<Argument>,
        chosen: Vec<Chosen>,
    ) {
        if arguments.is_empty() && chosen.is_empty() {
            return;
        }
        let call = (!arguments.is_empty()).then(|| Called {
            name: name.to_string(),
            at,
            arguments,
        });
        self.typing.note(self.node, call, chosen);
    }

    /// Checked in range before its grid is sized from it, then noted and registered.
    fn solver(
        &mut self,
        name: &str,
        numbers: &[f64],
        view: &[(&str, f64)],
        span: ByteSpan,
        chosen: Vec<Chosen>,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let [first, ..] = numbers else {
            return Err(EngineError::BadArity(name.to_string()));
        };
        let params = super::solvers::params(name, *first, view);
        if !params.valid() {
            return Err(EngineError::refused(crate::error::Diagnostic {
                code: "engine.physics_out_of_range".to_string(),
                message: format!("`{name}` was given an argument outside the range it models"),
                location: crate::error::Located::at(name, Some(span)),
                help: "`sva-cli builtins` names every argument each solver takes".to_string(),
            }));
        }
        let handed = super::solvers::handed(&params)
            .into_iter()
            .enumerate()
            .map(|(at, (key, value))| Argument {
                written: at == 0 || view.iter().any(|(k, _)| *k == key),
                name: key,
                value,
            })
            .collect();
        self.note_call(name, span, handed, chosen);
        let value = Value::Solver(Box::new(params));
        let ty = Ty::discrete(Held::Sampled, Codomain::Real);
        Ok(Piece::Value(self.register(value, ty, var)))
    }

    /// A number a call reads as an argument. A grid count is not one: `1sp` names no
    /// cutoff, so the same fold that reads a delay answers `None` for it here.
    pub(super) fn named_value(&self, e: &Expr, cx: Cx) -> Option<f64> {
        crate::loops::plain(crate::loops::amount(self.inst, e, cx)?)
    }

    /// `rand(key, seed=)` is a keyed hash, read at whatever the key names. A constant key is
    /// one number, folded here. A key that moves with `t` is a piecewise-constant closed form in `t`:
    /// closed in `t`, so every instant hashes the key its own time names.
    fn drawn(
        &mut self,
        positional: &[&Expr],
        named: &[(&str, f64)],
        cx: Cx,
        span: ByteSpan,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let (key, seed) = match positional {
            [] => {
                let seed = named_or(named, "seed", 0.0) as u64;
                let drawn = hash::keyed("", seed) as f64 / u64::MAX as f64;
                return Ok(Piece::ClosedForm(Body::Const(C64::real(drawn))));
            }
            [key] => (*key, named_or(named, "seed", 0.0)),
            [key, seed, ..] => (
                *key,
                self.named_value(seed, cx)
                    .ok_or_else(|| EngineError::BadArity("rand".to_string()))?,
            ),
        };
        let seed = seed as u64;
        let Piece::ClosedForm(body) = self.walk(key, cx, var)? else {
            return Err(self.refused_at(
                "type.samples_in_closed_form",
                "`rand` reads a key this node already sampled.".to_string(),
                "hash a closed form in t, or read the buffer where it is written",
                Some(span),
            ));
        };
        let body = crate::refs::fold_constants(self.typing, &body);
        if let Some(n) = super::constant_value(&body, var) {
            return Ok(Piece::ClosedForm(Body::Const(C64::real(hash::draw(
                seed, n,
            )))));
        }
        let of = self.part(body, Some(span));
        Ok(Piece::ClosedForm(Body::Keyed { seed, of }))
    }
}

/// FORMAT 3.3's arithmetic row: a closed form of its operands alone, so one image serves a
/// lowered call and a folded constant.
pub(super) fn numeric(name: &str) -> bool {
    Unary::from_name(name).is_some() || matches!(name, "max" | "min" | "pow")
}

/// The closed form a numeric call names, built once for the lowering and for the constant fold, so
/// an argument folded before a call and the same argument lowered agree by construction.
pub(super) fn arithmetic(
    name: &str,
    bodies: &[Body],
    named: &[(&str, f64)],
    var: Var,
    origin: Origin,
) -> Option<Body> {
    let part = |f: Body| Part::new(origin, f);
    let unary = |op: Unary, x: &Body| Body::Apply(op, part(x.clone()));
    let fold =
        |op: Fold, a: &Body, b: &Body| Body::Fold(op, vec![part(a.clone()), part(b.clone())]);
    if name == "sat" {
        let [x] = bodies else {
            return None;
        };
        let drive = Body::Const(C64::real(named_or(named, "drive", 1.0)));
        let driven = Body::Mul(vec![part(x.clone()), part(drive)]);
        return Some(Body::Apply(Unary::Sat, part(driven)));
    }
    if let ([x], Some(op)) = (bodies, Unary::from_name(name)) {
        return Some(unary(op, x));
    }
    Some(match (name, bodies) {
        ("max", [a, b]) => fold(Fold::Max, a, b),
        ("min", [a, b]) => fold(Fold::Min, a, b),
        ("pow", [base, exponent]) => return power(base, exponent, var, origin),
        _ => return None,
    })
}

/// An integer exponent is a polynomial factor and stays one. A real exponent is
/// `exp(x*ln(b))`, which only a positive base has: a constant one folds here, a signal one
/// becomes that exponential, and anything else has no value to approximate.
fn power(base: &Body, exponent: &Body, var: Var, origin: Origin) -> Option<Body> {
    let part = |f: Body| Part::new(origin, f);
    let n = super::constant_value(exponent, var);
    if let Some(n) = n.filter(|n| whole(*n)) {
        return Some(Body::Pow(part(base.clone()), n as i32));
    }
    let b = super::constant_value(base, var).filter(|b| *b > 0.0)?;
    if let Some(n) = n {
        return Some(Body::Const(C64::real(b.powf(n))));
    }
    let rate = Body::Const(C64::real(b.ln()));
    let scaled = Body::Mul(vec![part(exponent.clone()), part(rate)]);
    Some(Body::Apply(Unary::Exp, part(scaled)))
}

/// Whether a real exponent names a polynomial power an atom's u16 order holds.
fn whole(n: f64) -> bool {
    n.fract() == 0.0 && n.abs() <= f64::from(u16::MAX)
}

fn shoulders_of(named: &[(&str, f64)]) -> (f64, f64) {
    (named_or(named, "rise", 0.0), named_or(named, "fall", 0.0))
}

/// A named argument's number, or what the builtin takes when the call left it out.
pub(super) fn named_or(named: &[(&str, f64)], key: &str, fallback: f64) -> f64 {
    named
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(fallback, |(_, v)| *v)
}

/// Every positional argument as a number, in written order. A positional this cannot fold
/// is not skipped: skipping one would move every argument after it into the wrong field.
fn positional_values(
    low: &Lowering,
    args: &[Arg],
    cx: Cx,
    chosen: &mut Vec<Chosen>,
) -> Option<Vec<f64>> {
    args.iter()
        .filter_map(|a| match a {
            Arg::Pos(x) => Some(low.chosen_value(x, cx, chosen)),
            Arg::Named(..) => None,
        })
        .collect()
}

fn written_named(named: &[(String, f64)]) -> Vec<Argument> {
    named
        .iter()
        .map(|(name, value)| Argument {
            name: name.clone(),
            value: *value,
            written: true,
        })
        .collect()
}

/// A modal bank's positionals under the names its signature gives them, then its named ones.
fn positional_named(name: &str, numbers: &[f64], named: &[(String, f64)]) -> Vec<Argument> {
    let params = crate::overload::signature(name).map_or(&[][..], |s| s.params);
    let positional = params.iter().zip(numbers).map(|(p, value)| Argument {
        name: p.name.to_string(),
        value: *value,
        written: true,
    });
    positional.chain(written_named(named)).collect()
}
