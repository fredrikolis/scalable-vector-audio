// Concern: advises where a window sits wholly inside a crop's own shoulder | Non-concern: every other lint rule (lint.rs), what a crop multiplies | IO: (&Graph, roots) -> Vec<Finding>

use std::collections::BTreeSet;

use sva_ast::Graph;
use sva_core::{LintCode, Severity};
use sva_engine::{NodeId, Typing, Value};
use sva_formula::closed_form::children;
use sva_formula::{Body, Edge};

use crate::lint::Finding;

struct Swallowed {
    window_secs: f64,
    ramp_secs: f64,
    at_secs: f64,
    gain_db: f64,
}

/// A ramp reaches zero at its own edge, and nothing refuses a window written under one.
pub fn window_findings(graph: &Graph, roots: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut said = BTreeSet::new();
    for root in roots {
        let Ok(typing) = sva_engine::types(graph, root) else {
            continue;
        };
        for (path, id) in typing.paths() {
            let Value::ClosedForm(form) = typing.value(id) else {
                continue;
            };
            let mut found = Vec::new();
            ramped(&typing, &form.body, 0.0, &mut found);
            for held in found {
                let message = format!(
                    "`{path}` holds a {:.4} s window at {:.4} s inside its own {:.4} s ramp, \
                     which is under half amplitude there: {:.1} dB at the window's loudest \
                     instant",
                    held.window_secs, held.at_secs, held.ramp_secs, held.gain_db
                );
                if said.insert(format!("{path}|{message}")) {
                    out.push(Finding {
                        code: LintCode::WindowInsideRamp,
                        severity: Severity::Advice,
                        subject: path.to_string(),
                        message,
                        line: None,
                    });
                }
            }
        }
    }
    out
}

fn ramped(typing: &Typing, f: &Body, shift: f64, out: &mut Vec<Swallowed>) {
    // A warp reads its operand at a time no shift states, so nothing under one is placed.
    if matches!(f, Body::Warp { .. }) {
        return;
    }
    if let Body::Crop {
        of,
        l,
        r,
        rise,
        fall,
    } = f
    {
        let mut under = Vec::new();
        windows(typing, &of.body, shift, &mut BTreeSet::new(), &mut under);
        for ramp in ramps(*l, *r, *rise, *fall, shift) {
            for (at, ends) in under.iter().copied() {
                // Past the ramp's half turn a window is over half amplitude: shaping.
                let turn = ramp.turn(ramp.loudest(at, ends));
                if at >= ramp.from && ends <= ramp.to && turn < 0.5 {
                    out.push(Swallowed {
                        window_secs: ends - at,
                        ramp_secs: ramp.secs(),
                        at_secs: at,
                        gain_db: 20.0 * Ramp::gain(turn).max(f64::MIN_POSITIVE).log10(),
                    });
                }
            }
        }
    }
    let under = under_shift(f, shift);
    for part in children(f) {
        ramped(typing, &part.body, under, out);
    }
}

fn ramps(l: Edge, r: Edge, rise: f64, fall: f64, shift: f64) -> Vec<Ramp> {
    let mut out = Vec::new();
    if let Some(a) = finite(l)
        && rise > 0.0
    {
        out.push(Ramp {
            from: a + shift,
            to: a + rise + shift,
            rising: true,
        });
    }
    if let Some(b) = finite(r)
        && fall > 0.0
    {
        out.push(Ramp {
            from: b - fall + shift,
            to: b + shift,
            rising: false,
        });
    }
    out
}

struct Ramp {
    from: f64,
    to: f64,
    rising: bool,
}

impl Ramp {
    fn secs(&self) -> f64 {
        self.to - self.from
    }

    /// 0 at the ramp's silent edge, 1 at its full one.
    fn turn(&self, at: f64) -> f64 {
        let held = match self.rising {
            true => (at - self.from) / self.secs(),
            false => (self.to - at) / self.secs(),
        };
        held.clamp(0.0, 1.0)
    }

    fn gain(turn: f64) -> f64 {
        0.5 - 0.5 * (std::f64::consts::PI * turn).cos()
    }

    /// A window's loudest instant here: its far edge from the silent one.
    fn loudest(&self, at: f64, ends: f64) -> f64 {
        match self.rising {
            true => ends,
            false => at,
        }
    }
}

fn windows(
    typing: &Typing,
    f: &Body,
    shift: f64,
    seen: &mut BTreeSet<NodeId>,
    out: &mut Vec<(f64, f64)>,
) {
    if matches!(f, Body::Warp { .. }) {
        return;
    }
    if let Body::Node(id) = f {
        if !seen.insert(*id) {
            return;
        }
        if let Value::ClosedForm(form) = typing.value(*id) {
            windows(typing, &form.body, shift, seen, out);
        }
        return;
    }
    if let Body::Crop { l, r, .. } = f
        && let (Some(a), Some(b)) = (finite(*l), finite(*r))
        && b > a
    {
        out.push((a + shift, b + shift));
    }
    let under = under_shift(f, shift);
    for part in children(f) {
        windows(typing, &part.body, under, seen, out);
    }
}

/// `x(t - by)` reads `x`'s frame `by` late, so every edge in it lands that late.
fn under_shift(f: &Body, shift: f64) -> f64 {
    match f {
        Body::Shift { by, .. } => shift + by,
        _ => shift,
    }
}

fn finite(e: Edge) -> Option<f64> {
    matches!(e, Edge::At(_)).then(|| e.value())
}
