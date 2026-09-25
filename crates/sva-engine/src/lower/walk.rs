// Concern: the recursion over one node's written expression | Non-concern: what a call lowers to (calls.rs), classifying a loop (loops.rs) | IO: (&Expr, Cx, Var) -> a Piece

use sva_ast::{Arg, BinOp, ByteSpan, Expr, Literal};
use sva_formula::{Body, C64, Held, IndexId, NodeId, Ty, Var, note};

use crate::error::EngineError;
use crate::instantiate::{Cx, Node, RELEASE};
use crate::loops::{self, Shift, shift_of};
use crate::lower::{Lowering, Piece, SelfMode, constant};
use crate::offset::Offset;
use crate::overload;
use crate::typing::Value;

impl Lowering<'_, '_> {
    pub(super) fn walk(&mut self, e: &Expr, cx: Cx, var: Var) -> Result<Piece, EngineError> {
        if self.never.holds(e, cx) {
            return Ok(Piece::ClosedForm(Body::Const(C64::ZERO)));
        }
        let inst = self.inst;
        if let Some(r) = inst.follow(e, cx, |e2, cx2| self.walk(e2, cx2, var)) {
            return r;
        }
        match inst.node(e, cx) {
            Node::Lit(l) => self.literal(l, var),
            Node::Name(name) => self.name(name, var),
            Node::Bin(op, l, r) => self.binary(op, l, r, cx, var),
            Node::Read { path, arg, span } => self.read(path, arg, span, cx, var),
            Node::Own { arg, span } => self.own(arg, span, cx, var),
            Node::Call { name, args, span } => self.call(name, args, span, cx, var),
        }
    }

    /// The series expands around a body whose own refs are already inline. A zero
    /// coefficient is no loop at all, and a series around it would take the log of zero.
    pub(super) fn expand(
        &mut self,
        piece: Piece,
        var: Var,
        gain: sva_formula::C64,
        delay: f64,
    ) -> Result<Piece, EngineError> {
        if gain.is_zero() {
            return Ok(piece);
        }
        let Some(inline) = self.inlined(&piece, var).map(|body| pruned(body, var)) else {
            return Err(self.refused(
                "engine.series_body_not_inlinable",
                "a closed loop expands its body once per term, and this body holds a value \
                 no term can carry"
                    .to_string(),
                "collapse the body with sample(...) and write self(t - 1sp) for a sampled loop",
            ));
        };
        let index = self.index();
        Ok(Piece::ClosedForm(loops::neumann(
            &inline, gain, delay, index,
        )))
    }

    fn inlined(&self, piece: &Piece, var: Var) -> Option<Body> {
        match piece {
            Piece::ClosedForm(rest) => self.inline(rest, var),
            Piece::Value(_) => None,
        }
    }

    fn inline(&self, f: &Body, var: Var) -> Option<Body> {
        loops::expandable(f, var, &|id| match self.typing.value(id) {
            Value::ClosedForm(form) => Some((form.body.clone(), form.var)),
            _ => None,
        })
    }

    /// Fresh across the graph: two nodes' series compose into one closed form, and an index one of
    /// them reused would be captured by the other's expansion.
    pub(super) fn index(&mut self) -> IndexId {
        self.typing.next_index()
    }

    /// Reading the node's own output: the zero a series expands around, or one read on the
    /// grid, which is what makes the whole node discrete. Two call sites at two delays are
    /// two reads, never one.
    fn own(
        &mut self,
        arg: &Expr,
        span: sva_ast::ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        if self.mode == SelfMode::Zero {
            return Ok(Piece::ClosedForm(Body::Const(C64::ZERO)));
        }
        let tap = loops::tap_of(self.inst, arg, cx);
        let loops::Tap::At(delay) = tap else {
            return Err(loops::tap_refusal(tap, self.here(Some(span)))
                .expect("a tap that is not a delay names its reason"));
        };
        if let Some((_, id)) = self.own.iter().find(|(held, _)| *held == delay) {
            return Ok(Piece::Value(*id));
        }
        let ty = Ty::discrete(Held::Sampled, sva_formula::Codomain::Real);
        let id = self.register(Value::SelfAt(delay), ty, var);
        self.own.push((delay, id));
        Ok(Piece::Value(id))
    }

    fn literal(&mut self, l: &Literal, var: Var) -> Result<Piece, EngineError> {
        match l {
            Literal::Num(n) => Ok(Piece::ClosedForm(Body::Const(C64::real(*n)))),
            Literal::Bars(_) if var == Var::F => Err(self.refused(
                "type.bars_in_frequency",
                "a bar is a duration and has no position in f".to_string(),
                "write the duration in seconds, or move it into the closed form in t",
            )),
            Literal::Bars(_) => Err(EngineError::UnresolvedBars(self.here(None))),
            Literal::Samples(n) => {
                let ty = Ty::discrete(Held::Sampled, sva_formula::Codomain::Real);
                Ok(Piece::Value(self.register(Value::Grid(*n), ty, var)))
            }
            Literal::Str(s) => match note::frequency(s) {
                Some(hz) => Ok(Piece::ClosedForm(Body::Const(C64::real(hz)))),
                None => Err(self.refused(
                    "grammar.unknown_name",
                    format!("`{s}` is text, and only a note name reads as a frequency"),
                    "write a note name like `A4`, or the hertz itself",
                )),
            },
        }
    }

    fn name(&mut self, name: &str, var: Var) -> Result<Piece, EngineError> {
        if let Some((_, index)) = self.indices.iter().find(|(k, _)| k == name) {
            return Ok(Piece::ClosedForm(Body::Index(*index)));
        }
        let constant = match name {
            "t" if var == Var::T => return Ok(Piece::ClosedForm(Body::Line)),
            "f" if var == Var::F => return Ok(Piece::ClosedForm(Body::Line)),
            "t" | "f" => {
                return Err(self.refused(
                    "type.domain_mismatch",
                    format!("`{name}` is not the variable this closed form is written in."),
                    "write fourier on the t side, or ifourier on the f side",
                ));
            }
            "pi" => C64::real(std::f64::consts::PI),
            RELEASE => C64::real(f64::INFINITY),
            "i" => C64::new(0.0, 1.0),
            "inf" => {
                return Err(self.refused(
                    "grammar.inf_out_of_place",
                    "inf is a bound, not a value".to_string(),
                    "write inf only as the upper bound of a sum",
                ));
            }
            other => match note::frequency(other) {
                Some(hz) => C64::real(hz),
                None => return Err(EngineError::UnknownBuiltin(other.to_string())),
            },
        };
        Ok(Piece::ClosedForm(Body::Const(constant)))
    }

    fn binary(
        &mut self,
        op: BinOp,
        l: &Expr,
        r: &Expr,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let left = self.walk(l, cx, var)?;
        let right = self.walk(r, cx, var)?;
        if let (Piece::ClosedForm(a), Piece::ClosedForm(b)) = (&left, &right) {
            let (a, b) = (a.clone(), b.clone());
            let a = self.part(a, None);
            let b = self.part(b, None);
            return Ok(Piece::ClosedForm(match op {
                BinOp::Add => Body::Add(vec![a, b]),
                BinOp::Sub => {
                    let minus = self.part(Body::Const(C64::real(-1.0)), None);
                    let negated = self.part(Body::Mul(vec![minus, b]), None);
                    Body::Add(vec![a, negated])
                }
                BinOp::Mul => Body::Mul(vec![a, b]),
                BinOp::Div => Body::Div(a, b),
                BinOp::Mod => Body::Fold(sva_formula::Fold::Mod, vec![a, b]),
            }));
        }
        let name = match op {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
        };
        self.operation(name, vec![left, right], None, var)
    }

    /// A call whose operands did not all stay inside one closed form is a sampled operation: the
    /// signal operands decide its type, and a folded constant is neutral by FORMAT 3.3.
    pub(super) fn operation(
        &mut self,
        name: &str,
        pieces: Vec<Piece>,
        span: Option<ByteSpan>,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let params = overload::signature(name).map(|s| s.params).unwrap_or(&[]);
        let mut args = Vec::with_capacity(pieces.len());
        let mut signals = Vec::new();
        for (at, piece) in pieces.into_iter().enumerate() {
            let constant = matches!(&piece, Piece::ClosedForm(f) if constant::is_constant(f));
            if !constant
                && params
                    .get(at)
                    .is_some_and(|p| p.kind == overload::ParamKind::Scalar)
            {
                return Err(self.refused_at(
                    "engine.non_constant_argument",
                    format!(
                        "`{name}` reads `{}` as a number, and this one moves.",
                        params[at].name
                    ),
                    "write a number there, or move the expression into the signal",
                    span,
                ));
            }
            let id = self.seal(piece, var, None)?;
            if !constant {
                signals.push(self.typing.ty(id).read_on(var));
            }
            args.push(id);
        }
        let ty = overload::resolve(name, &signals)
            .map(|ty| ty.read_on(var))
            .map_err(|m| self.refuse(name, &m, span))?;
        let value = Value::Op {
            name: name.to_string(),
            args,
        };
        Ok(Piece::Value(self.register(value, ty, var)))
    }

    pub(super) fn read(
        &mut self,
        path: &str,
        arg: &Expr,
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let id = self
            .typing
            .id(path)
            .ok_or_else(|| EngineError::UnknownNode(path.to_string()))?;
        let Some(shift) = shift_of(self.inst, arg, cx) else {
            return self.warped(path, id, arg, span, cx, var);
        };
        let ty = self.typing.ty(id);
        let site = self.typing.mark(self.here(Some(span)));
        let sampled = |slf: &mut Self, held, at| {
            let value = Value::Read {
                source: id,
                at,
                site,
            };
            let read = Ty {
                held,
                dual: ty.dual && held.is_closed_form(),
                ..ty
            };
            Ok(Piece::Value(slf.register(value, read, var)))
        };
        let whole_steps = |steps: f64| match steps.fract() == 0.0 {
            true => Ok(Offset::Steps(-(steps as i64))),
            false => Err(self.refused_at(
                "ref.fractional_shift_on_samples",
                format!("`@{path}` is read {steps} samples back, which is not a whole one."),
                "write a whole number of sp, or the offset in seconds",
                Some(span),
            )),
        };
        if !ty.is_closed_form() {
            let at = match shift {
                Shift::Now => Offset::Steps(0),
                Shift::Steps(steps) => whole_steps(steps)?,
                Shift::Secs(by) => Offset::Secs(-by),
            };
            return sampled(self, ty.held, at);
        }
        match shift {
            Shift::Now => Ok(Piece::ClosedForm(Body::Node(id))),
            Shift::Secs(by) => {
                let of = self.part(Body::Node(id), Some(span));
                Ok(Piece::ClosedForm(Body::Shift { by, of }))
            }
            Shift::Steps(steps) => sampled(self, Held::Sampled, whole_steps(steps)?),
        }
    }

    /// A callee read at a time no offset names is the callee's closed form at that time, which is a
    /// pair only where the time is affine and a point-sampled closed form otherwise.
    fn warped(
        &mut self,
        path: &str,
        id: NodeId,
        arg: &Expr,
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        if !self.typing.ty(id).is_closed_form() {
            return Err(self.per_lane_on_samples(path, id, arg, span));
        }
        let Piece::ClosedForm(when) = self.walk(arg, cx, var)? else {
            return Err(self.unreadable(path, span));
        };
        let at = self.part(when, Some(span));
        let of = self.part(Body::Node(id), Some(span));
        Ok(Piece::ClosedForm(Body::Warp { at, of }))
    }

    /// A time written one per component is a per-lane substitution, which a closed form takes and a
    /// buffer does not: one read carries one offset, so the lanes are read one at a time.
    fn per_lane_on_samples(
        &self,
        path: &str,
        id: NodeId,
        arg: &Expr,
        span: ByteSpan,
    ) -> EngineError {
        let Some(lanes) = per_lane(arg) else {
            return self.unreadable(path, span);
        };
        let written: Vec<String> = lanes.iter().map(|e| sva_ast::render_expr(e)).collect();
        let width = usize::from(self.typing.ty(id).width);
        let reads = |read: &dyn Fn(usize, &str) -> String| {
            written
                .iter()
                .enumerate()
                .map(|(at, when)| read(at, when))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let help = match width {
            1 => format!(
                "read the buffer once per time: join({})",
                reads(&|_, when| format!("@{path}({when})"))
            ),
            w if w == written.len() => format!(
                "read the buffer once per component: join({})",
                reads(&|at, when| format!("ch(@{path}({when}), {at})"))
            ),
            w => format!(
                "`@{path}` has {w} components and this read names {} times; write one time \
                 per component",
                written.len()
            ),
        };
        self.refused_at(
            "engine.per_lane_read_on_samples",
            format!(
                "`@{path}` is samples and this read names one time per component: `{}`",
                written.join("`, `")
            ),
            &help,
            Some(span),
        )
    }

    fn unreadable(&self, path: &str, span: ByteSpan) -> EngineError {
        self.refused_at(
            "engine.unreadable_shift",
            format!("`@{path}` is read at a time this engine cannot settle"),
            "write the read at `t`, or at `t` minus a constant",
            Some(span),
        )
    }
}

/// The times a `join` names, one per component, or nothing where the argument names one time.
fn per_lane(arg: &Expr) -> Option<Vec<&Expr>> {
    let Expr::Call { name, args, .. } = arg else {
        return None;
    };
    if name != "join" || args.is_empty() {
        return None;
    }
    args.iter()
        .map(|a| match a {
            Arg::Pos(x) => Some(x),
            Arg::Named(..) => None,
        })
        .collect()
}

/// The `self` term of a series body folds to zero, and a zero addend is not a wave any
/// line enumeration can read: it goes before the series is built.
fn pruned(body: Body, var: Var) -> Body {
    let Body::Add(parts) = body else {
        return body;
    };
    let kept: Vec<sva_formula::Part> = parts
        .into_iter()
        .filter(|p| constant::constant_value(&p.body, var) != Some(0.0))
        .collect();
    match kept.as_slice() {
        [] => Body::Const(C64::ZERO),
        [only] => (*only.body).clone(),
        _ => Body::Add(kept),
    }
}
