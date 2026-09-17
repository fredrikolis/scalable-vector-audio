// Concern: the node a written cast or a filter becomes | Non-concern: the type each produces (cast.rs, overload.rs) | IO: (Cast or Shape, args) -> a Piece

use std::f64::consts::TAU;

use sva_ast::{Arg, ByteSpan, Expr};
use sva_formula::filter::{Shape, design};
use sva_formula::{Body, C64, Held, NodeId, Rational, Var};

use crate::cast::{Cast, Mismatch};
use crate::error::EngineError;
use crate::instantiate::Cx;
use crate::lower::{Lowering, Piece};
use crate::typing::Value;

impl Lowering<'_, '_> {
    pub(super) fn cast(
        &mut self,
        cast: Cast,
        args: &[Arg],
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        cast.check_arguments()
            .map_err(|m| self.refuse(cast.name(), &m, Some(span)))?;
        let Some(Arg::Pos(x)) = args.first() else {
            return Err(EngineError::BadArity(cast.name().to_string()));
        };
        let inner = match cast {
            Cast::Fourier => Var::T,
            Cast::IFourier => Var::F,
            _ => var,
        };
        let piece = self.walk(x, cx, inner)?;
        let source = self.seal(piece, inner, None)?;
        let ty = cast
            .resolve(&[self.typing.ty(source)])
            .map_err(|m| self.blocked(cast.name(), m, source, span))?;
        let out = match cast {
            Cast::Fourier => Var::F,
            Cast::IFourier => Var::T,
            _ => inner,
        };
        let id = self.register(Value::Cast(cast, source), ty, out);
        // A retyping cast holds a closed form, so it composes into the closed form around it.
        Ok(match ty.is_closed_form() {
            true => Piece::ClosedForm(Body::Node(id)),
            false => Piece::Value(id),
        })
    }

    /// A retyping cast that refused names the atom that left A, which only normalizing the
    /// operand can find.
    fn blocked(
        &mut self,
        call: &str,
        mismatch: Mismatch,
        source: sva_formula::NodeId,
        span: ByteSpan,
    ) -> EngineError {
        let left = match self.typing.value(source) {
            Value::ClosedForm(form) => sva_formula::normalize_closed_form(form).err(),
            _ => None,
        };
        let mismatch = match left {
            Some(left) => {
                let at = self.typing.locate(left.origin);
                mismatch.blocked_by(
                    left.origin,
                    format!(
                        "{} at {at}: {}",
                        left.sketch.describe(),
                        left.reason.clause()
                    ),
                )
            }
            None => mismatch,
        };
        self.refuse(call, &mismatch, Some(span))
    }

    /// On a pair the response multiplies the dual, so a filter written in `t` is the three
    /// steps FORMAT 7.3 writes by hand.
    pub(super) fn filter(
        &mut self,
        shape: Shape,
        args: &[Arg],
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let positional: Vec<&Expr> = args
            .iter()
            .filter_map(|a| match a {
                Arg::Pos(x) => Some(x),
                Arg::Named(..) => None,
            })
            .collect();
        let [signal, rest @ ..] = positional.as_slice() else {
            return Err(EngineError::BadArity(shape.name().to_string()));
        };
        let written = |slot: usize, key: &str| -> Option<&Expr> {
            rest.get(slot).copied().or_else(|| {
                args.iter().find_map(|a| match a {
                    Arg::Named(k, x) if k == key => Some(x),
                    _ => None,
                })
            })
        };
        let arguments = [written(0, "cutoff"), written(1, "q"), written(2, "gain")];

        let piece = self.walk(signal, cx, var)?;
        let source = self.seal(piece, var, None)?;
        let ty = self.typing.ty(source);
        if ty.held == Held::Sampled {
            let Some(cutoff) = arguments[0] else {
                return Err(EngineError::BadArity(shape.name().to_string()));
            };
            let mut held = [self.automation(cutoff, span, cx, var)?; 3];
            for (slot, fallback) in [(1, shape.default_q()), (2, 0.0)] {
                held[slot] = match arguments[slot] {
                    Some(x) => self.automation(x, span, cx, var)?,
                    None => self.constant_node(fallback, var),
                };
            }
            let value = Value::Filter {
                shape,
                x: source,
                cutoff: held[0],
                q: held[1],
                gain: held[2],
            };
            return Ok(Piece::Value(self.register(value, ty, var)));
        }
        let at = |slot: usize, fallback: f64| -> Option<f64> {
            match arguments[slot] {
                Some(x) => self.named_value(x, cx),
                None => Some(fallback),
            }
        };
        let Some(cutoff) = at(0, f64::NAN) else {
            return Err(self.swept(shape, span));
        };
        let Some(q) = at(1, shape.default_q()) else {
            return Err(self.swept(shape, span));
        };
        let Some(gain) = at(2, 0.0) else {
            return Err(self.swept(shape, span));
        };
        if !cutoff.is_finite() {
            return Err(EngineError::BadArity(shape.name().to_string()));
        }
        if !ty.has_dual() {
            let m = Mismatch::new(
                "type.filter_needs_a_dual",
                &[ty],
                "write the filter over sample(x) to filter at the render rate",
            );
            return Err(self.refuse(shape.name(), &m, Some(span)));
        }
        let response = Body::Rational(in_frequency(design(shape, cutoff, q, gain)));
        let spectrum = match var {
            Var::F => source,
            Var::T => self.register(
                Value::Cast(Cast::Fourier, source),
                ty.read_on(Var::F),
                Var::F,
            ),
        };
        let left = self.part(Body::Node(spectrum), Some(span));
        let right = self.part(response, Some(span));
        let filtered = self.seal(
            Piece::ClosedForm(Body::Mul(vec![left, right])),
            Var::F,
            None,
        )?;
        match var {
            Var::F => Ok(Piece::Value(filtered)),
            Var::T => {
                let ty = self.typing.ty(filtered).read_on(Var::T);
                Ok(Piece::Value(self.register(
                    Value::Cast(Cast::IFourier, filtered),
                    ty,
                    Var::T,
                )))
            }
        }
    }

    /// A parameter the recurrence reads once a sample: a number stays a number, and a closed form
    /// of `t` is collapsed on the grid the filter itself runs on.
    fn automation(
        &mut self,
        written: &Expr,
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<NodeId, EngineError> {
        let piece = self.walk(written, cx, var)?;
        let id = self.seal(piece, var, None)?;
        let held = self.typing.ty(id);
        let settled = held.held == Held::Sampled
            || matches!(self.typing.value(id), Value::ClosedForm(form)
                if super::constant_value(&form.body, form.var).is_some());
        if settled {
            return Ok(id);
        }
        let ty = Cast::Sample
            .resolve(&[held])
            .map_err(|m| self.refuse(Cast::Sample.name(), &m, Some(span)))?;
        Ok(self.register(Value::Cast(Cast::Sample, id), ty, var))
    }

    /// A filter argument is a node like any other operand, so the renderer reads it the same way.
    fn constant_node(&mut self, value: f64, var: Var) -> NodeId {
        let piece = Piece::ClosedForm(Body::Const(C64::real(value)));
        self.seal(piece, var, None)
            .expect("a constant closed form always types")
    }

    fn swept(&self, shape: Shape, span: ByteSpan) -> EngineError {
        let m = Mismatch::new(
            "type.filter_needs_a_dual",
            &[],
            "a swept argument is a sampled filter: write it over sample(x)",
        );
        self.refuse(shape.name(), &m, Some(span))
    }
}

/// `H(s)` on `s = 2*pi*i*f`: each root in `s` sits at `z/(2*pi*i)` in `f`, and the factor
/// every root sheds collects in the gain.
fn in_frequency(r: Rational) -> Rational {
    let turn = C64::new(0.0, TAU);
    let back = turn.inv();
    let moved = |set: Vec<C64>| -> Vec<C64> { set.into_iter().map(|z| z * back).collect() };
    let net = r.zeros.len() as i32 - r.poles.len() as i32;
    let scale = match net >= 0 {
        true => turn.powi(net.unsigned_abs()),
        false => back.powi(net.unsigned_abs()),
    };
    Rational {
        gain: r.gain * scale,
        zeros: moved(r.zeros),
        poles: moved(r.poles),
    }
}
