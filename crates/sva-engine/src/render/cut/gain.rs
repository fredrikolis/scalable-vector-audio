// Concern: bounds how far a change at each node can move the output, from the root down | Non-concern: a node's own magnitude (bound/) | IO: (typing, root, bounds) -> each gain, or why none

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::{Body, Fold, NodeId, Unary};
use sva_samples::biquad::{clamp_cutoff, clamp_q, design};

use crate::cast::Cast;
use crate::error::EngineError;
use crate::loops::Delay;
use crate::render::bound::{Bounds, Ringing};
use crate::render::extent::Supports;
use crate::schedule::holds_self;
use crate::typing::{Typing, Value};

/// A bound on `sup |change at the output| / sup |change here|`, or what stops one.
pub(super) type Gain = Result<f64, String>;

/// `perturbable` are the nodes a cut may change; a product may only carry one changed factor,
/// since the other factors' own peaks are what bound it. `window` is the seconds a factor's
/// peak is taken over.
pub(super) struct Gains<'b, 'a> {
    pub(super) tys: &'b Typing,
    pub(super) bounds: &'b Bounds<'a>,
    pub(super) supports: &'b Supports<'a>,
    pub(super) perturbable: &'b BTreeSet<NodeId>,
    pub(super) window: (f64, f64),
    under: BTreeMap<NodeId, bool>,
}

impl<'b, 'a> Gains<'b, 'a> {
    pub(super) fn new(
        tys: &'b Typing,
        bounds: &'b Bounds<'a>,
        supports: &'b Supports<'a>,
        perturbable: &'b BTreeSet<NodeId>,
        window: (f64, f64),
    ) -> Self {
        Gains {
            tys,
            bounds,
            supports,
            perturbable,
            window,
            under: BTreeMap::new(),
        }
    }

    /// Every node the root reaches, its gain the sum over each reader of the reader's own
    /// times what the reader does to it. A loop hands its gain to what feeds it, past its own
    /// members.
    pub(super) fn from(&mut self, root: NodeId) -> Result<BTreeMap<NodeId, Gain>, EngineError> {
        let mut order = Vec::new();
        topological(self.tys, root, &mut BTreeSet::new(), &mut order);
        let mut held: BTreeMap<NodeId, Gain> = BTreeMap::from([(root, Ok(1.0))]);
        for &id in order.iter().rev() {
            let Some(own) = held.get(&id).cloned() else {
                continue;
            };
            let locals = match holds_self(self.tys, id, &mut BTreeSet::new()) {
                true => self.looped(id)?,
                false => self.locals(id)?,
            };
            for (operand, local) in locals {
                let reached = own.clone().and_then(|g| local.map(|l| g * l));
                let slot = held.entry(operand).or_insert(Ok(0.0));
                *slot = match (slot.clone(), reached) {
                    (Ok(a), Ok(b)) => Ok(a + b),
                    (Err(why), _) | (_, Err(why)) => Err(why),
                };
            }
        }
        Ok(held
            .into_iter()
            .map(|(id, gain)| {
                let finite = gain.and_then(|g| match g.is_finite() {
                    true => Ok(g),
                    false => Err("a factor with no finite peak".to_string()),
                });
                (id, finite)
            })
            .collect())
    }

    /// Whether a cut anywhere under `id` can change it.
    fn changes(&mut self, id: NodeId) -> bool {
        if let Some(held) = self.under.get(&id) {
            return *held;
        }
        self.under.insert(id, false);
        let found = self.perturbable.contains(&id)
            || operands(self.tys, id).into_iter().any(|o| self.changes(o));
        self.under.insert(id, found);
        found
    }

    fn peak(&mut self, id: NodeId) -> Result<Gain, EngineError> {
        Ok(self
            .bounds
            .peak(id)
            .ok_or_else(|| "a factor with no bound".to_string()))
    }

    /// What `id` does to each operand's change, on its own.
    fn locals(&mut self, id: NodeId) -> Result<Vec<(NodeId, Gain)>, EngineError> {
        let one = |ids: &[NodeId], g: Gain| ids.iter().map(|o| (*o, g.clone())).collect();
        Ok(match self.tys.value(id).clone() {
            Value::ClosedForm(form) => {
                let mut out = Vec::new();
                for node in crate::refs::nodes_in(&form.body) {
                    out.push((node, self.written(&form.body, node)));
                }
                out
            }
            Value::Cast(Cast::Sample, source) => one(&[source], Ok(1.0)),
            Value::Cast(_, source) => one(&[source], Err("a transform of the whole signal".into())),
            Value::Read { source, at, .. } => match at.steps_at(self.bounds.config().rate) {
                Ok(_) => one(&[source], Ok(1.0)),
                Err(_) => one(&[source], Err("a read between two samples".into())),
            },
            Value::Filter {
                shape,
                x,
                cutoff,
                q,
                gain,
            } => {
                let numbers = [cutoff, q, gain].map(|p| constant(self.tys, p));
                let rate = f64::from(self.bounds.config().rate);
                let summed = match numbers {
                    [Some(c), Some(q), Some(g)] => {
                        let coeffs = design(shape, clamp_cutoff(c, rate).0, clamp_q(q).0, g, rate);
                        Ringing::of(&coeffs)
                            .map(|r| r.sum)
                            .ok_or_else(|| "a filter that never settles".to_string())
                    }
                    _ => Err("a filter whose coefficients move".to_string()),
                };
                let mut out = vec![(x, summed)];
                out.extend(one(
                    &[cutoff, q, gain],
                    Err("a filter's coefficient".into()),
                ));
                out
            }
            Value::Op { name, args } => self.operation(&name, &args)?,
            Value::SelfAt(_) | Value::Grid(_) | Value::Solver(_) => Vec::new(),
        })
    }

    fn operation(
        &mut self,
        name: &str,
        args: &[NodeId],
    ) -> Result<Vec<(NodeId, Gain)>, EngineError> {
        let rest = |g: Gain| args.iter().skip(1).map(move |a| (*a, g.clone()));
        let divisor = || match args.get(1).and_then(|d| constant(self.tys, *d)) {
            Some(d) if d != 0.0 => Ok(1.0 / d.abs()),
            _ => Err("a division by a moving signal".to_string()),
        };
        Ok(match name {
            "+" | "-" | "join" | "max" | "min" | "ch" => {
                args.iter().map(|a| (*a, Ok(1.0))).collect()
            }
            "crop" => std::iter::once((args[0], Ok(1.0)))
                .chain(rest(Err("a crop's edge".into())))
                .collect(),
            "/" => std::iter::once((args[0], divisor()))
                .chain(rest(Err("a divisor".into())))
                .collect(),
            "pow" => {
                let whole = match args.get(1).and_then(|n| constant(self.tys, *n)) {
                    Some(1.0) => Ok(1.0),
                    _ => Err("a power of a signal".to_string()),
                };
                std::iter::once((args[0], whole))
                    .chain(rest(Err("an exponent".into())))
                    .collect()
            }
            "*" => {
                let mut out = Vec::with_capacity(args.len());
                for (i, &arg) in args.iter().enumerate() {
                    let mut g: Gain = Ok(1.0);
                    for (j, &other) in args.iter().enumerate() {
                        if i == j {
                            continue;
                        }
                        let both = self.changes(arg) && self.changes(other);
                        let peak = match both {
                            true => Err("a product of two signals a cut may change".to_string()),
                            false => self.peak(other)?,
                        };
                        g = g.and_then(|g| peak.map(|p| g * p));
                    }
                    out.push((arg, g));
                }
                out
            }
            unary => match Unary::from_name(unary) {
                Some(Unary::Sin | Unary::Cos | Unary::Tanh | Unary::Sat | Unary::Abs) => {
                    vec![(args[0], Ok(1.0))]
                }
                _ => args
                    .iter()
                    .map(|a| (*a, Err(format!("`{unary}` of a signal"))))
                    .collect(),
            },
        })
    }

    /// A loop `y = F + g y(t - d)`: a change in what feeds `F` moves `y` by `1 / (1 - g)`
    /// of what it moves `F` by, where every tap's gain sums under one.
    fn looped(&mut self, owner: NodeId) -> Result<Vec<(NodeId, Gain)>, EngineError> {
        let (leaves, taps) = self.affine(owner)?;
        Ok(leaves
            .into_iter()
            .map(|(leaf, g)| {
                let through = taps.clone().and_then(|t| match t < 1.0 {
                    true => Ok(1.0 / (1.0 - t)),
                    false => Err("a loop whose gain does not contract".to_string()),
                });
                (leaf, g.and_then(|g| through.map(|k| g * k)))
            })
            .collect())
    }

    /// The loop body as each fed operand's gain into `F`, and the taps' summed gain.
    fn affine(&mut self, id: NodeId) -> Result<(Vec<(NodeId, Gain)>, Gain), EngineError> {
        if !holds_self(self.tys, id, &mut BTreeSet::new()) {
            return Ok((vec![(id, Ok(1.0))], Ok(0.0)));
        }
        let Value::Op { name, args } = self.tys.value(id).clone() else {
            let taps = match self.tys.value(id) {
                Value::SelfAt(Delay::Varying) => Err("a loop whose delay moves".to_string()),
                Value::SelfAt(_) => Ok(1.0),
                _ => Err("a loop through a filter or a transform".to_string()),
            };
            return Ok((Vec::new(), taps));
        };
        let looped: Vec<usize> = (0..args.len())
            .filter(|at| holds_self(self.tys, args[*at], &mut BTreeSet::new()))
            .collect();
        match name.as_str() {
            "+" | "-" => {
                let (mut leaves, mut taps) = (Vec::new(), Ok(0.0));
                for arg in args {
                    let (more, t) = self.affine(arg)?;
                    leaves.extend(more);
                    taps = taps.and_then(|a| t.map(|b| a + b));
                }
                Ok((leaves, taps))
            }
            "crop" | "tanh" | "sat" | "sin" | "abs" => self.affine(args[0]),
            "*" | "/" if looped.len() == 1 && (name == "*" || looped[0] == 0) => {
                let at = looped[0];
                let mut scale: Gain = Ok(1.0);
                let mut leaves = Vec::new();
                for (j, &other) in args.iter().enumerate() {
                    if j == at {
                        continue;
                    }
                    let factor = match (name.as_str(), self.changes(other)) {
                        (_, true) => Err("a loop scaled by a signal a cut may change".into()),
                        ("*", false) => self.peak(other)?,
                        _ => match constant(self.tys, other) {
                            Some(d) if d != 0.0 => Ok(1.0 / d.abs()),
                            _ => Err("a loop divided by a moving signal".to_string()),
                        },
                    };
                    scale = scale.and_then(|s| factor.map(|f| s * f));
                    leaves.push((other, Err("a factor of a loop's own feedback".into())));
                }
                let (inner, taps) = self.affine(args[at])?;
                let scaled = |g: Gain| g.and_then(|g| scale.clone().map(|s| g * s));
                leaves.extend(inner.into_iter().map(|(leaf, g)| (leaf, scaled(g))));
                Ok((leaves, scaled(taps)))
            }
            _ => Ok((
                Vec::new(),
                Err("a loop through a map with no contraction".to_string()),
            )),
        }
    }

    /// A written form's gain on `node`, constructor by constructor.
    fn written(&mut self, body: &Body, node: NodeId) -> Gain {
        if !crate::refs::nodes_in(body).contains(&node) {
            return Ok(0.0);
        }
        let parts = sva_formula::closed_form::children;
        match body {
            Body::Node(_) => Ok(1.0),
            Body::Add(_) | Body::Join(_) => parts(body)
                .iter()
                .try_fold(0.0, |held, p| Ok(held + self.written(&p.body, node)?)),
            Body::Fold(Fold::Max | Fold::Min, _) => parts(body)
                .iter()
                .try_fold(0.0f64, |held, p| Ok(held.max(self.written(&p.body, node)?))),
            Body::Mul(factors) => {
                let mut g = 1.0;
                let mut changed = 0;
                for factor in factors {
                    if self.body_changes(&factor.body) {
                        changed += 1;
                        g *= self.written(&factor.body, node)?;
                    } else {
                        g *= self.body_peak(&factor.body)?;
                    }
                }
                match changed {
                    1 => Ok(g),
                    _ => Err("a product of two signals a cut may change".to_string()),
                }
            }
            Body::Div(num, den) => match &*den.body {
                Body::Const(c) if !c.is_zero() => {
                    Ok(self.written(&num.body, node)? / (c.re.abs() + c.im.abs()))
                }
                _ => Err("a division by a moving signal".to_string()),
            },
            Body::Pow(base, 1) => self.written(&base.body, node),
            Body::Apply(Unary::Sin | Unary::Cos | Unary::Tanh | Unary::Sat | Unary::Abs, of)
            | Body::Shift { of, .. }
            | Body::Crop { of, .. }
            | Body::Channel(of, _) => self.written(&of.body, node),
            Body::Warp { at, of } if crate::refs::nodes_in(&at.body).is_empty() => {
                self.written(&of.body, node)
            }
            _ => Err("a closed form that reads it through a nonlinear term".to_string()),
        }
    }

    fn body_changes(&mut self, body: &Body) -> bool {
        crate::refs::nodes_in(body)
            .into_iter()
            .any(|id| self.changes(id))
    }

    /// The largest a written factor reaches over the render's window.
    fn body_peak(&mut self, body: &Body) -> Gain {
        let (lo, hi) = self.window;
        if let Body::Node(id) = body {
            return self.peak(*id).map_err(|e| e.to_string())?;
        }
        self.supports
            .bound(body, lo, hi, 0)
            .ok_or_else(|| "a factor with no bound".to_string())
    }
}

/// Every node a value reads directly.
pub(super) fn operands(tys: &Typing, id: NodeId) -> Vec<NodeId> {
    match tys.value(id) {
        Value::ClosedForm(form) => crate::refs::nodes_in(&form.body),
        Value::Cast(_, source) | Value::Read { source, .. } => vec![*source],
        Value::Op { args, .. } => args.clone(),
        Value::Filter {
            x, cutoff, q, gain, ..
        } => vec![*x, *cutoff, *q, *gain],
        Value::SelfAt(_) | Value::Grid(_) | Value::Solver(_) => Vec::new(),
    }
}

/// Operands before readers.
pub(super) fn topological(
    tys: &Typing,
    id: NodeId,
    seen: &mut BTreeSet<NodeId>,
    out: &mut Vec<NodeId>,
) {
    if !seen.insert(id) {
        return;
    }
    for operand in operands(tys, id) {
        topological(tys, operand, seen, out);
    }
    out.push(id);
}

fn constant(tys: &Typing, id: NodeId) -> Option<f64> {
    match tys.value(id) {
        Value::ClosedForm(form) => crate::lower::constant_value(&form.body, form.var),
        _ => None,
    }
}
