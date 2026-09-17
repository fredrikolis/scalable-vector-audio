// Concern: the series each named waveform is, and the series a written sum lowers to | Non-concern: the atoms a plain call lowers to (calls.rs) | IO: (name, args) -> a Piece

use std::f64::consts::{PI, TAU};

use sva_ast::{Arg, ByteSpan, Expr};
use sva_formula::{Body, Bound, C64, IndexId, Part, Series, Unary, Var};

use crate::error::EngineError;
use crate::instantiate::{Cx, Node};
use crate::lower::{Lowering, Piece};
use crate::vocabulary::SERIES;

impl Lowering<'_, '_> {
    /// `saw`, `square` and `triangle` carry no harmonic budget: each is the series FORMAT 6.4
    /// states, truncated once at collapse.
    pub(super) fn wave(
        &mut self,
        name: &str,
        positional: &[&Expr],
        named: &[(&str, f64)],
        cx: Cx,
        var: Var,
    ) -> Result<Option<Piece>, EngineError> {
        let odd = match name {
            "saw" => false,
            "square" | "triangle" => true,
            _ => return Ok(None),
        };
        let [hz, ..] = positional else {
            return Err(EngineError::BadArity(name.to_string()));
        };
        let Piece::ClosedForm(hz) = self.walk(hz, cx, var)? else {
            return Err(EngineError::BadArity(name.to_string()));
        };
        let phase = match positional.get(1) {
            Some(x) => match self.walk(x, cx, var)? {
                Piece::ClosedForm(f) => f,
                Piece::Value(_) => return Err(EngineError::BadArity(name.to_string())),
            },
            None => Body::Const(C64::real(
                named
                    .iter()
                    .find(|(k, _)| *k == "phase")
                    .map_or(0.0, |(_, v)| *v),
            )),
        };
        let index = self.index();
        let ordinal = self.ordinal(index, odd, name);
        let term = self.partial(name, &hz, &phase, &ordinal, index);
        let scale = self.part(Body::Const(C64::real(amplitude(name))), None);
        let series = self.part(
            Body::Series(Box::new(Series {
                index,
                lo: 1,
                hi: Bound::Infinite,
                term,
            })),
            None,
        );
        Ok(Some(Piece::ClosedForm(Body::Mul(vec![scale, series]))))
    }

    /// `k` for every harmonic, `2k-1` where only the odd ones are present.
    fn ordinal(&mut self, index: IndexId, odd: bool, _name: &str) -> Body {
        if !odd {
            return Body::Index(index);
        }
        let two = self.part(Body::Const(C64::real(2.0)), None);
        let k = self.part(Body::Index(index), None);
        let doubled = self.part(Body::Mul(vec![two, k]), None);
        let minus = self.part(Body::Const(C64::real(-1.0)), None);
        Body::Add(vec![doubled, minus])
    }

    /// One partial: `sin(2*pi*n*hz*t + phase)/n^p`, with a half-turn of phase per index
    /// where the series alternates.
    fn partial(
        &mut self,
        name: &str,
        hz: &Body,
        phase: &Body,
        ordinal: &Body,
        index: IndexId,
    ) -> Part {
        let turn = self.part(Body::Const(C64::real(TAU)), None);
        let n = self.part(ordinal.clone(), None);
        let hz = self.part(hz.clone(), None);
        let line = self.part(Body::Line, None);
        let angle = self.part(Body::Mul(vec![turn, n, hz, line]), None);
        let mut sum = vec![angle, self.part(phase.clone(), None)];
        if name == "triangle" {
            let half = self.part(Body::Const(C64::real(PI)), None);
            let k = self.part(Body::Index(index), None);
            let one = self.part(Body::Const(C64::real(-1.0)), None);
            let shifted = self.part(Body::Add(vec![k, one]), None);
            sum.push(self.part(Body::Mul(vec![half, shifted]), None));
        }
        let wave = self.part(Body::Apply(Unary::Sin, self.part_of(Body::Add(sum))), None);
        let power = if name == "triangle" { 2 } else { 1 };
        let denominator = self.part(Body::Pow(self.part_of(ordinal.clone()), power), None);
        Part::new(wave.origin, Body::Div(wave, denominator))
    }

    fn part_of(&self, f: Body) -> Part {
        Part::bare(f)
    }

    pub(super) fn sum(
        &mut self,
        args: &[Arg],
        span: ByteSpan,
        cx: Cx,
        var: Var,
    ) -> Result<Piece, EngineError> {
        let [
            Arg::Pos(Expr::Var(name)),
            Arg::Pos(lo),
            Arg::Pos(hi),
            Arg::Pos(body),
        ] = args
        else {
            return Err(EngineError::BadArity(SERIES.to_string()));
        };
        let Some(lo) = self.named_value(lo, cx) else {
            return Err(EngineError::BadArity(SERIES.to_string()));
        };
        let hi = match self.inst.node(hi, cx) {
            Node::Name("inf") => Bound::Infinite,
            _ => match self.named_value(hi, cx) {
                Some(n) => Bound::Finite(n as i64),
                None => return Err(EngineError::BadArity(SERIES.to_string())),
            },
        };
        let index = self.index();
        self.indices.push((name.clone(), index));
        let walked = self.walk(body, cx, var);
        self.indices.pop();
        let Piece::ClosedForm(term) = walked? else {
            return Err(EngineError::BadArity(SERIES.to_string()));
        };
        let term = self.part(term, Some(span));
        Ok(Piece::ClosedForm(Body::Series(Box::new(Series {
            index,
            lo: lo as i64,
            hi,
            term,
        }))))
    }
}

fn amplitude(name: &str) -> f64 {
    match name {
        "square" => 4.0 / PI,
        "triangle" => 8.0 / (PI * PI),
        _ => 2.0 / PI,
    }
}
