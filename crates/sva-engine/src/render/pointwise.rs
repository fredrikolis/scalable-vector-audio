// Concern: point-samples a closed form whose operand crossed a cast, one instant at a time | Non-concern: the rows a spectral sum takes (sva-samples) | IO: (NodeId) -> a Buffer + a measured Label

use std::collections::BTreeMap;

use sva_formula::{Body, C64, NodeId, SpectralSum, Var};
use sva_samples::{
    ALIAS_OVERSAMPLE, AliasScore, Audible, Buffer, CollapseError, Detail, Label, Refs, Rule,
    Source, crop_gain, eval_spectral_sum_at, eval_written_at, lane_of, measure_alias,
    truncate_spectral_sum, truncate_written, unary,
};

use crate::error::{Diagnostic, EngineError, Located};
use crate::refs;
use crate::render::Render;
use crate::typing::Value;

/// What one subterm answers with, decided once: normalizing a closed form at every instant of a
/// horizon is the same answer as many times as there are samples.
enum Point {
    SpectralSum(Box<SpectralSum>),
    Written {
        body: Box<Body>,
        refs: BTreeMap<NodeId, Point>,
    },
    Operation {
        name: String,
        args: Vec<Point>,
        widths: Vec<usize>,
    },
    Buffer(NodeId),
}

/// FORMAT 9.1 row 4 over the value tree rather than over one written closed form: the operands of a
/// closed form with no dual may have crossed a cast, and a crossing has no atoms to compose.
pub fn point_sample(
    held: &Render,
    id: NodeId,
    score: AliasScore,
) -> Result<(Buffer, Label), EngineError> {
    let tree = plan(held, id)?;
    let rate = held.config.rate;
    let len = held
        .config
        .horizon
        .len(rate)
        .map_err(|e| refused(held, id, &e))?;
    let width = usize::from(held.tys.ty(id).width);
    let mut planes = Vec::with_capacity(width);
    for component in 0..width.max(1) {
        planes.push(sweep(held, id, &tree, component, len, 1)?);
    }
    let start_secs = held.config.horizon.start_secs;
    let alias_db = match score {
        AliasScore::Asked => {
            let high = sweep(held, id, &tree, 0, len * ALIAS_OVERSAMPLE, ALIAS_OVERSAMPLE)?;
            Some(
                measure_alias(
                    &planes[0],
                    &high,
                    ALIAS_OVERSAMPLE,
                    f64::from(rate),
                    start_secs,
                )
                .asr_db,
            )
        }
        AliasScore::NotAsked => None,
    };
    let mut buffer = Buffer::of_planes(rate, planes);
    buffer.origin_secs = held.config.horizon.start_secs;
    let detail = Detail::Point {
        rule: Rule::PointSampled,
        alias_db,
    };
    Ok((
        buffer,
        Label::new(Source::Measured, held.config.profile.name, rate, detail),
    ))
}

fn sweep(
    held: &Render,
    id: NodeId,
    tree: &Point,
    component: usize,
    len: usize,
    oversample: usize,
) -> Result<Vec<f64>, EngineError> {
    let step = 1.0 / (f64::from(held.config.rate) * oversample as f64);
    let origin = held.config.horizon.start_secs;
    (0..len)
        .map(|i| {
            value(held, tree, component, origin + i as f64 * step)
                .map(|v| v.re)
                .map_err(|e| refused(held, id, &e))
        })
        .collect()
}

fn value(held: &Render, tree: &Point, component: usize, t: f64) -> Result<C64, CollapseError> {
    match tree {
        Point::SpectralSum(sum) => eval_spectral_sum_at(sum, component, t),
        Point::Written { body, refs } => eval_written_at(
            body,
            component,
            t,
            &Reads {
                held,
                planned: refs,
            },
        ),
        Point::Operation { name, args, widths } => {
            operation(held, name, args, widths, component, t)
        }
        Point::Buffer(id) => Ok(read(held, *id, component, t)),
    }
}

/// What a written closed form's `Body::Node` reads: the plan this render made for that node, and
/// the width its type states.
struct Reads<'a> {
    held: &'a Render,
    planned: &'a BTreeMap<NodeId, Point>,
}

impl Refs for Reads<'_> {
    fn value(&self, id: NodeId, component: usize, t: f64) -> Result<C64, CollapseError> {
        value(self.held, &self.planned[&id], component, t)
    }

    fn width(&self, id: NodeId) -> usize {
        usize::from(self.held.tys.ty(id).width)
    }
}

/// A sampled operand has no value between its own grid points, so the nearest one answers,
/// and none at all off either end, which is zero.
fn read(held: &Render, id: NodeId, component: usize, t: f64) -> C64 {
    let buffer = held.buffers.get(&id).expect("a read is materialized first");
    let at = ((t - buffer.origin_secs) * f64::from(buffer.rate)).round();
    if at < 0.0 {
        return C64::ZERO;
    }
    let plane = buffer.plane(component.min(buffer.width.saturating_sub(1)));
    C64::real(plane.get(at as usize).copied().unwrap_or(0.0))
}

fn operation(
    held: &Render,
    name: &str,
    args: &[Point],
    widths: &[usize],
    component: usize,
    t: f64,
) -> Result<C64, CollapseError> {
    if name == "join" {
        let (at, inner) = lane_of(widths, component)
            .ok_or(CollapseError::NotEvaluable("a component past the width"))?;
        return value(held, &args[at], inner, t);
    }
    if name == "ch" {
        let k = value(held, &args[1], 0, t)?.re.round();
        let width = widths.first().copied().unwrap_or(1);
        let k = usize::try_from(k as i64).unwrap_or(usize::MAX);
        if k >= width {
            return Err(CollapseError::NotEvaluable("a component past the width"));
        }
        return value(held, &args[0], k, t);
    }
    let mut held_args = Vec::with_capacity(args.len());
    for arg in args {
        held_args.push(value(held, arg, component, t)?);
    }
    let pair = || (held_args[0], held_args[1]);
    let first = || held_args[0];
    Ok(match name {
        "+" => held_args.iter().fold(C64::ZERO, |a, b| a + *b),
        "*" => held_args.iter().fold(C64::ONE, |a, b| a * *b),
        "-" => pair().0 - pair().1,
        "/" => pair().0 / pair().1,
        "%" => C64::real(pair().0.re.rem_euclid(pair().1.re)),
        "max" => C64::real(pair().0.re.max(pair().1.re)),
        "min" => C64::real(pair().0.re.min(pair().1.re)),
        "pow" => power(pair().0, pair().1.re)?,
        "crop" => {
            let shoulder = |at: usize| held_args.get(at).map_or(0.0, |s| s.re);
            let (a, b) = (held_args[1].re, held_args[2].re);
            match crop_gain(t, a, b, shoulder(3), shoulder(4)) {
                0.0 => C64::ZERO,
                gain => first().scale(gain),
            }
        }
        _ => match sva_formula::Unary::from_name(name) {
            Some(op) => unary(op, first()),
            None => return Err(CollapseError::NotEvaluable("this operation")),
        },
    })
}

fn power(x: C64, exponent: f64) -> Result<C64, CollapseError> {
    let whole = exponent.round();
    if (exponent - whole).abs() > f64::EPSILON {
        return Ok(C64::real(x.re.powf(exponent)));
    }
    if whole.abs() > f64::from(u16::MAX) {
        return Err(CollapseError::NotEvaluable("a whole power past 65535"));
    }
    Ok(match whole >= 0.0 {
        true => x.powi(whole as u32),
        false => x.powi((-whole) as u32).inv(),
    })
}

fn plan(held: &Render, id: NodeId) -> Result<Point, EngineError> {
    if !held.tys.ty(id).is_closed_form() {
        return Ok(Point::Buffer(id));
    }
    let band = Audible::of(&held.config.profile, held.config.rate);
    let left = match refs::spectral_sum_of(&held.tys, id, Var::T) {
        Ok(sum) => {
            let held = truncate_spectral_sum(&sum, band).map_err(|e| refused(held, id, &e))?;
            return Ok(Point::SpectralSum(Box::new(held)));
        }
        Err(left) => left,
    };
    match held.tys.value(id) {
        Value::Op { name, args } => {
            let mut set = Vec::with_capacity(args.len());
            for arg in args {
                set.push(plan(held, *arg)?);
            }
            Ok(Point::Operation {
                name: name.clone(),
                args: set,
                widths: args
                    .iter()
                    .map(|arg| usize::from(held.tys.ty(*arg).width))
                    .collect(),
            })
        }
        Value::ClosedForm(form) if form.var == Var::T => {
            let mut refs = BTreeMap::new();
            for node in refs::nodes_in(&form.body) {
                refs.insert(node, plan(held, node)?);
            }
            let body = truncate_written(&form.body, band).map_err(|e| refused(held, id, &e))?;
            Ok(Point::Written {
                body: Box::new(body),
                refs,
            })
        }
        _ => Err(left),
    }
}

fn refused(held: &Render, id: NodeId, e: &CollapseError) -> EngineError {
    EngineError::refused(Diagnostic {
        code: e.code().to_string(),
        message: e.to_string(),
        location: Located::at(held.tys.name(id), None),
        help: "write the subterm inside sample(...), or give the observation a window".to_string(),
    })
}
