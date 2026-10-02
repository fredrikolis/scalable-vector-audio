// Concern: the recursion over one node's written expression | Non-concern: what a call lowers to (calls.rs), classifying a loop (loops.rs) | IO: (&Expr, Cx, Var) -> a Piece

use sva_ast::{Address, Arg, BinOp, ByteSpan, Expr, Literal};
use sva_formula::{Body, C64, Held, IndexId, NodeId, Ty, Var};

use crate::cast::Cast;
use crate::error::{EngineError, Located};
use crate::instantiate::{Cx, Node, Thunk};
use crate::loops::{self, Tap};
use crate::lower::{Index, Lowering, Piece, SelfMode, constant, on};
use crate::overload;
use crate::time::{Affine, Lattice, Q};
use crate::typing::{Step, Value, When};

impl Lowering<'_> {
    pub(super) fn walk(&mut self, e: &Expr, cx: Cx, var: Var) -> Result<Piece, EngineError> {
        let inst = self.inst;
        if let Some(r) = inst.follow(e, cx, |e2, cx2| self.walk(e2, cx2, var)) {
            return r;
        }
        match inst.node(e, cx) {
            Node::Lit(l) => self.literal(l, var),
            Node::Name(name) => self.name(name, var),
            Node::Bin(op, l, r) => self.binary(op, l, r, cx, var),
            Node::Read {
                path,
                arg,
                address: Address::Time,
                span,
            } => self.read(path, arg, span, cx, var),
            Node::Read {
                path,
                arg,
                address: Address::Index,
                span,
            } => self.indexed(path, arg, span, cx, var),
            Node::Own { arg, address, span } => self.own((arg, address), span, cx, var),
            Node::Signal { of, arg, span, .. } => self.signal(of, arg, span, cx, var),
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
                "collapse the body with sample(...) and read self by index, as self[idx(t) - 1], \
                 for a discrete loop",
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

    /// The zero a series expands around, or a discrete loop's past by index. Which sample a
    /// recurrence reads is part of its meaning; `self(e)` names an instant, on a sample only at
    /// some rates, and typing knows no rate. So it is a type error, as `x[t]` is.
    fn own(
        &mut self,
        (arg, address): (&Expr, Address),
        span: sva_ast::ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let why = match &self.mode {
            SelfMode::Zero => return Ok(Piece::ClosedForm(Body::Const(C64::ZERO))),
            SelfMode::Discrete(why) => why,
            SelfMode::Absent => unreachable!("a node that reads itself is a series or discrete"),
        };
        if address == Address::Time {
            let written = |arg: Expr, address| {
                sva_ast::render_expr(&Expr::SelfRef {
                    arg: Box::new(arg),
                    address,
                    span,
                })
            };
            let indexed = Expr::Call {
                name: sva_ast::INDEX.to_string(),
                args: vec![Arg::Pos(arg.clone())],
                span,
            };
            return Err(self.refused_at(
                "type.discrete_self_at_time",
                format!(
                    "`{}` is a discrete loop, made so by {why}: it steps from sample to \
                     sample, and `{}` names an instant rather than a sample, on one at some \
                     rates and between two at others",
                    self.node,
                    written(arg.clone(), Address::Time)
                ),
                &format!(
                    "read its past by index, as {}",
                    written(indexed, Address::Index)
                ),
                Some(span),
            ));
        }
        let tap = loops::tap_of(self.inst, arg, address, cx);
        let at = match tap {
            Tap::Back(delay) => {
                if let Some((_, id)) = self.own.iter().find(|(held, _)| *held == delay) {
                    return Ok(Piece::Value(*id));
                }
                let back = Affine {
                    scale: Q::ONE,
                    shift: delay.neg(),
                };
                When::At(back)
            }
            Tap::Moving => unreachable!("an index moves as an index, never as a time"),
            Tap::Indexed => self.sample_index(arg, span, cx)?,
            refused => {
                return Err(loops::tap_refusal(refused, self.here(Some(span)))
                    .expect("a tap that is not a delay names its reason"));
            }
        };
        let ty = Ty::discrete(Held::Sampled, sva_formula::Codomain::Real);
        let id = self.register(Value::SelfAt { at }, ty, var);
        if let Tap::Back(delay) = tap {
            self.own.push((delay, id));
        }
        Ok(Piece::Value(id))
    }

    /// A time that moves with `t`, a node of its own the reading evaluates each sample.
    pub(super) fn time(&mut self, arg: &Expr, cx: Cx) -> Result<NodeId, EngineError> {
        let piece = self.walk(arg, cx, Var::T)?;
        self.seal(piece, Var::T, None)
    }

    fn literal(&mut self, l: &Literal, var: Var) -> Result<Piece, EngineError> {
        let grid = self.grid;
        match l {
            Literal::Num(n) => Ok(Piece::ClosedForm(Body::Const(C64::real(*n)))),
            Literal::Bars(_) if var == Var::F => Err(self.refused(
                "type.bars_in_frequency",
                "a bar is a duration and has no position in f".to_string(),
                "write the duration in seconds, or move it into the closed form in t",
            )),
            Literal::Bars(_) => Err(EngineError::UnresolvedBars(self.here(None))),
            Literal::Samples(n) if grid.is_rate() => {
                let count = self.part(Body::Const(C64::real(*n)), None);
                let rate = self.part(Body::Const(C64::real(grid.rate.into())), None);
                Ok(Piece::ClosedForm(Body::Div(count, rate)))
            }
            Literal::Samples(n) => {
                let exact = Q::decimal(*n).and_then(|n| grid.steps(n));
                let secs = exact.map_or_else(|| grid.steps_f64(*n), Q::to_f64);
                Ok(Piece::ClosedForm(Body::Const(C64::real(secs))))
            }
            Literal::Str(s) => match crate::vocabulary::note_hz(s) {
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
        if let Some((_, index)) = self.indices.iter().rev().find(|(k, _)| k == name) {
            return Ok(Piece::ClosedForm(match *index {
                Index::Series(index) => Body::Index(index),
                Index::Term(k) => Body::Const(C64::real(k as f64)),
            }));
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
            "i" => C64::new(0.0, 1.0),
            "inf" => C64::real(f64::INFINITY),
            other => match crate::vocabulary::note_hz(other) {
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
        let (left, right) = match (left, right) {
            (Piece::ClosedForm(a), Piece::ClosedForm(b)) => {
                let a = self.part(a, None);
                let b = self.part(b, None);
                let body = match op {
                    BinOp::Add => Body::Add(vec![a, b]),
                    BinOp::Sub => {
                        let minus = self.part(Body::Const(C64::real(-1.0)), None);
                        let negated = self.part(Body::Mul(vec![minus, b]), None);
                        Body::Add(vec![a, negated])
                    }
                    BinOp::Mul => Body::Mul(vec![a, b]),
                    BinOp::Div => Body::Div(a, b),
                    BinOp::Mod => Body::Fold(sva_formula::Fold::Mod, vec![a, b]),
                };
                return self.folded(body, var);
            }
            pieces => pieces,
        };
        let name = match op {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
        };
        self.operation(name, vec![left, right], None, var)
    }

    /// A body of numbers holding `inf` is the one number it folds to, and `inf - inf` or
    /// `0*inf` refuse; so is a sum of `inf` and terms finite at every instant, as `t - inf` is
    /// `-inf`. Anything else stays the body it was built as.
    pub(super) fn folded(&self, body: Body, var: Var) -> Result<Piece, EngineError> {
        let reach = (var == Var::T).then(|| i64::MAX as f64 / self.grid.sr());
        let swamped = || reach.and_then(|reach| constant::swamped(&body, reach));
        match constant::unbounded(&body).or_else(swamped) {
            Some(v) if v.is_nan() => Err(self
                .infinite("arithmetic on inf folds to no number here, as inf - inf or 0*inf does")),
            Some(v) => Ok(Piece::ClosedForm(Body::Const(C64::real(v)))),
            None => Ok(Piece::ClosedForm(body)),
        }
    }

    pub(super) fn infinite(&self, what: &str) -> EngineError {
        self.refused(
            "engine.infinite_value",
            what.to_string(),
            "write inf only where it names a number: a crop's edge, a parameter a crop \
             reads, or a constant it folds away in",
        )
    }

    /// A call whose operands did not all stay inside one closed form is a sampled operation: the
    /// signal operands decide its representation, a folded constant is neutral in it by FORMAT
    /// 3.3, and every operand counts toward its width.
    pub(super) fn operation(
        &mut self,
        name: &str,
        pieces: Vec<Piece>,
        span: Option<ByteSpan>,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let params = overload::signature(name).map(|s| s.params).unwrap_or(&[]);
        let mut args = Vec::with_capacity(pieces.len());
        for (at, piece) in pieces.into_iter().enumerate() {
            if matches!(&piece, Piece::ClosedForm(f) if constant::holds_infinite(f))
                && !(name == "crop" && (1..=2).contains(&at))
            {
                return Err(self.infinite(&format!("`{name}` reads inf as a signal")));
            }
            let written = matches!(&piece, Piece::ClosedForm(f) if constant::is_constant(f));
            let id = self.seal(piece, var, None)?;
            let constant = written || constant::number_of(self.typing, id).is_some();
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
            args.push((id, constant));
        }
        if name == sva_ast::CHANNEL
            && let [(of, false), (k, true)] = args.as_slice()
            && let x = self.typing.ty(*of)
            && let Some(k) = constant::number_of(self.typing, *k)
            && k >= f64::from(x.width)
            && !crate::schedule::holds_self(self.typing, *of, &mut Default::default())
        {
            return Err(self.refused_at(
                "type.width_mismatch",
                format!("component {k} of a value {} components wide", x.width),
                "read a component the value holds, counting from zero",
                span,
            ));
        }
        let ty = match overload::resolve(name, &self.operands(&args, var)) {
            Err(m) if m.code == "type.samples_in_closed_form" => {
                for (id, _) in &mut args {
                    *id = self.sampled(*id);
                }
                overload::resolve(name, &self.operands(&args, var))
            }
            held => held,
        }
        .map(|ty| ty.read_on(var))
        .map_err(|m| self.refuse(name, &m, span))?;
        let args = args.into_iter().map(|(id, _)| id).collect();
        let value = Value::Op {
            name: name.to_string(),
            args,
        };
        Ok(Piece::Value(self.register(value, ty, var)))
    }

    /// Each operand's type as the call reads it: a constant takes the first signal's
    /// representation and keeps its own width.
    fn operands(&self, args: &[(NodeId, bool)], var: Var) -> Vec<Ty> {
        let ty = |id: NodeId| self.typing.ty(id).read_on(var);
        let signal = args.iter().find(|(_, c)| !c).map(|&(id, _)| ty(id));
        args.iter()
            .map(|&(id, constant)| match (constant, signal) {
                (true, Some(s)) => Ty {
                    width: ty(id).width,
                    ..s
                },
                _ => ty(id),
            })
            .collect()
    }

    /// A closed form in `t` meeting samples is its collapse at the instants they are stepped at,
    /// the one crossing `sample(...)` writes; anything else stays as it is.
    fn sampled(&mut self, id: NodeId) -> NodeId {
        let ty = self.typing.ty(id);
        if ty.held != Held::Form(Var::T) || constant::number_of(self.typing, id).is_some() {
            return id;
        }
        let Ok(sampled) = Cast::Sample.resolve(&[ty]) else {
            return id;
        };
        self.register(Value::Cast(Cast::Sample, id), sampled, Var::T)
    }

    pub(super) fn read(
        &mut self,
        path: &str,
        arg: &Expr,
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let id = self.source(path)?;
        let closed = self.typing.ty(id).is_closed_form();
        let stateful = self.holds_state(id);
        let (id, at) = match loops::time_of(self.inst, arg, cx) {
            Some(time) if closed && time == Affine::NOW => {
                return Ok(Piece::ClosedForm(Body::Node(id)));
            }
            Some(time) if closed && time.scale == Q::ONE => {
                let of = self.part(Body::Node(id), Some(span));
                return Ok(Piece::ClosedForm(Body::Shift {
                    by: time.shift.neg().to_f64(),
                    of,
                }));
            }
            _ if closed => return self.warped(id, arg, span, cx, var),
            Some(time) => {
                let grid = self
                    .grid
                    .speed(time)
                    .ok_or_else(|| self.unplaced(path, span))?;
                (on(path, grid, self.inst, self.typing)?, When::At(time))
            }
            None => match per_lane(arg) {
                Some(_) => return Err(self.per_lane_on_samples(path, id, arg, span)),
                None if stateful => return Err(self.stateful_warp(path, span)),
                None => (id, When::Moving(self.time(arg, cx)?)),
            },
        };
        Ok(Piece::Value(self.reading(id, at, span, var)))
    }

    /// `x[i]` reads a stored sample; a closed form's samples are its collapse at the instants
    /// its reader steps at.
    fn indexed(
        &mut self,
        path: &str,
        arg: &Expr,
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let id = self.source(path)?;
        let at = self.sample_index(arg, span, cx)?;
        self.stepped(id, at, span, var)
    }

    /// One read of `id`'s samples at an index; a closed form's samples are its collapse at
    /// the instants its reader steps at, the one value every read of it shares.
    fn stepped(
        &mut self,
        id: NodeId,
        at: When,
        span: ByteSpan,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let ty = self.typing.ty(id);
        if ty.is_closed_form() {
            Cast::Sample
                .resolve(&[ty])
                .map_err(|m| self.refuse(Cast::Sample.name(), &m, Some(span)))?;
        }
        Ok(Piece::Value(self.reading(id, at, span, var)))
    }

    /// `x[i]` reads the signal `x` stands for as a ref's index read does: lowered once, at
    /// its own `t`, and read at step `i`. A signal that is one node read at its own `t` is that
    /// node.
    fn signal(
        &mut self,
        of: Thunk,
        arg: &Expr,
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let at = self.sample_index(arg, span, cx)?;
        let piece = self.walk(of.expr, self.inst.signal(of, cx), Var::T)?;
        let id = self.seal(piece, Var::T, None)?;
        let id = match self.typing.value(id) {
            Value::Read {
                source,
                at: When::At(time),
                ..
            } if *time == Affine::NOW && self.typing.grid(*source) == self.typing.grid(id) => {
                *source
            }
            _ => id,
        };
        self.stepped(id, at, span, var)
    }

    /// An index is an integer, which typing refuses otherwise: one rounded line plus a count,
    /// or any other integer, evaluated each sample.
    fn sample_index(&mut self, arg: &Expr, span: ByteSpan, cx: Cx) -> Result<When, EngineError> {
        if !crate::index::integer(self.inst, arg, cx) {
            return Err(self.refused_at(
                "type.non_integer_index",
                format!(
                    "`{}` is no integer: an index must be an integer; use idx(…)",
                    self.inst.render(arg, cx)
                ),
                "write idx(...) around a time, as @x[idx(t - 0.5b)], or a whole count",
                Some(span),
            ));
        }
        Ok(match crate::index::read(self.inst, arg, cx) {
            Some(index) => When::Index(index),
            None => When::Step(self.step(arg, cx)?),
        })
    }

    /// The integer `e` names, over what `index::integer` admits.
    fn step(&mut self, e: &Expr, cx: Cx) -> Result<Step, EngineError> {
        let inst = self.inst;
        if let Some(index) = crate::index::read(inst, e, cx) {
            return Ok(Step::Index(index));
        }
        if let Some(r) = inst.follow(e, cx, |e2, cx2| self.step(e2, cx2)) {
            return r;
        }
        match inst.node(e, cx) {
            Node::Call { name, args, .. } if name == sva_ast::INDEX => {
                let (Some(Arg::Pos(time)), Some(round)) =
                    (args.first(), crate::index::rounding(args.get(1)))
                else {
                    unreachable!("typing admits idx(time) and idx(time, floor or ceil) only")
                };
                Ok(Step::Nearest(self.time(time, cx)?, round))
            }
            Node::Bin(op, l, r) => {
                let (l, r) = (self.step(l, cx)?, self.step(r, cx)?);
                Ok(match op {
                    BinOp::Add => Step::Add(vec![l, r]),
                    BinOp::Sub => Step::Add(vec![l, Step::Neg(Box::new(r))]),
                    BinOp::Mul => Step::Mul(vec![l, r]),
                    BinOp::Div | BinOp::Mod => unreachable!("an integer holds no / or %"),
                })
            }
            _ => unreachable!("an integer is a whole literal, idx(...), or +, - and * of them"),
        }
    }

    /// A read whose constant lands past what a whole sample index holds.
    fn unplaced(&self, path: &str, span: ByteSpan) -> EngineError {
        self.refused_at(
            "engine.unreadable_position",
            format!("`@{path}` is read at a time past what a sample index holds"),
            "read it nearer t = 0",
            Some(span),
        )
    }

    /// A stateful node has a value only at the steps it takes, which a time that moves
    /// lands between.
    fn stateful_warp(&self, path: &str, span: ByteSpan) -> EngineError {
        let note = match self.construct(path, 0) {
            Some((what, at)) => format!("\nnote: `{path}` is stateful: {what} at {at}"),
            None => String::new(),
        };
        self.refused_at(
            "type.stateful_warp",
            format!("stateful `{path}` read at a moving time{note}"),
            &format!("read the nearest step: `@{path}[idx(…)]`"),
            Some(span),
        )
    }

    /// The first construct in `path`'s body that steps: a filter, a solver, `self`, an index
    /// read, or one inside a stateful node it reads.
    fn construct(&self, path: &str, depth: usize) -> Option<(String, Located)> {
        let (e, cx) = self.inst.at(path)?;
        let mut found = None;
        self.visit(e, cx, &mut |node| {
            let (what, span) = match node {
                Node::Call { name, span, .. }
                    if crate::vocabulary::shape(name).is_some()
                        || sva_ast::FINITE_DIFFERENCE.contains(&name) =>
                {
                    (format!("`{name}(…)`"), span)
                }
                Node::Own { address, span, .. } => (
                    match address {
                        Address::Time => "`self(…)`".to_string(),
                        Address::Index => "`self[…]`".to_string(),
                    },
                    span,
                ),
                Node::Read {
                    path: read,
                    address: Address::Index,
                    span,
                    ..
                } => (format!("`@{read}[…]`"), span),
                Node::Signal { name, span, .. } => (format!("`{name}[…]`"), span),
                Node::Read { path: read, .. }
                    if depth < 32
                        && self.typing.id(read).is_some_and(|id| self.holds_state(id)) =>
                {
                    found = self.construct(read, depth + 1);
                    return found.is_some();
                }
                _ => return false,
            };
            found = Some((what, Located::at(path, Some(span))));
            true
        });
        found
    }

    fn holds_state(&self, id: NodeId) -> bool {
        super::holds_state(self.typing, id)
    }

    /// Every node under `e`, first to last, until `stop` answers true.
    fn visit(&self, e: &Expr, cx: Cx, stop: &mut dyn FnMut(Node) -> bool) -> bool {
        let inst = self.inst;
        if let Some(r) = inst.follow(e, cx, |e2, cx2| self.visit(e2, cx2, stop)) {
            return r;
        }
        let node = inst.node(e, cx);
        if stop(node) {
            return true;
        }
        match node {
            Node::Lit(_) | Node::Name(_) => false,
            Node::Bin(_, l, r) => self.visit(l, cx, stop) || self.visit(r, cx, stop),
            Node::Own { arg, .. } | Node::Read { arg, .. } | Node::Signal { arg, .. } => {
                self.visit(arg, cx, stop)
            }
            Node::Call { args, .. } => args.iter().any(|a| {
                let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                self.visit(x, cx, stop)
            }),
        }
    }

    /// One read of `source`'s samples at `at`.
    pub(super) fn reading(&mut self, source: NodeId, at: When, span: ByteSpan, var: Var) -> NodeId {
        let ty = self.typing.ty(source);
        let site = self.typing.mark(self.node, Some(span));
        let read = Ty {
            held: Held::Sampled,
            dual: false,
            ..ty
        };
        self.register(Value::Read { source, at, site }, read, var)
    }

    /// A callee read at a time no line names is the callee's closed form at that time, which
    /// is a pair only where the time is affine and a point-sampled closed form otherwise; a
    /// time that is samples reads the callee's own samples there.
    fn warped(
        &mut self,
        id: NodeId,
        arg: &Expr,
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        match self.walk(arg, cx, var)? {
            Piece::ClosedForm(when) => {
                let at = self.part(when, Some(span));
                let of = self.part(Body::Node(id), Some(span));
                Ok(Piece::ClosedForm(Body::Warp { at, of }))
            }
            Piece::Value(time) => {
                let ty = Cast::Sample
                    .resolve(&[self.typing.ty(id)])
                    .map_err(|m| self.refuse(Cast::Sample.name(), &m, Some(span)))?;
                let sampled = self.register(Value::Cast(Cast::Sample, id), ty, var);
                Ok(Piece::Value(self.reading(
                    sampled,
                    When::Moving(time),
                    span,
                    var,
                )))
            }
        }
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
        let lanes = per_lane(arg).expect("a read one time per component");
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
