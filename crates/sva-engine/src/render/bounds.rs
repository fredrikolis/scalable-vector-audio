// Concern: each reading between lattice samples and its bound, a loop's over its extent, refused past precision | Non-concern: one reading's proof (sva-samples) | IO: (&Render) -> bounds or a refusal

use sva_formula::spectral_sum::sup::sup_from;
use sva_formula::{Held, NodeId, Var};
use sva_samples::{At, Bound, Extent, NodeRenderer, Slot, kernel};

use super::{Render, extent, pointwise, sampled};
use crate::error::{Diagnostic, EngineError, Located};
use crate::loops;
use crate::typing::{Gain, Typing, Value, When};

#[derive(Clone, Debug, PartialEq)]
pub struct Reconstruction {
    pub node: String,
    pub source: String,
    /// `shift`, `scale`, `moving`, `loop`, `point`, `constant` or `output`.
    pub reading: &'static str,
    pub bound: Bound,
    /// A linear loop's proven bound on its own output, relative to its full scale.
    pub looped: Option<f64>,
}

impl Render {
    pub fn reconstructions(&self) -> Vec<Reconstruction> {
        let Some(bound) = kernel().bound(self.config.profile.ceiling_hz, self.lattice()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &id in &self.schedule.materialize {
            match self.tys.ty(id).held {
                Held::Sampled => self.programmed(id, bound, &mut out),
                Held::Form(_) => {
                    if pointwise::plan(self, id).is_ok_and(|tree| pointwise::reads_samples(&tree)) {
                        out.push(self.named(id, id, "point", bound, None));
                    }
                }
                Held::Frames => {}
            }
        }
        for node in self.tys.drawn_between() {
            out.push(Reconstruction {
                node: node.clone(),
                source: "rand".to_string(),
                reading: "constant",
                bound,
                looped: None,
            });
        }
        let map = self.out_map();
        if self.output.is_some() && !(map.whole() && map.a == 1) {
            out.push(self.named(self.root, self.root, "output", bound, None));
        }
        let mut once: Vec<Reconstruction> = Vec::with_capacity(out.len());
        for r in out {
            if !once.contains(&r) {
                once.push(r);
            }
        }
        once
    }

    fn programmed(&self, id: NodeId, bound: Bound, out: &mut Vec<Reconstruction>) {
        let Ok(program) = sampled::program(self, id) else {
            return;
        };
        extent::leaves(&program.renderer, &mut |leaf| {
            let NodeRenderer::Read { slot, at } = leaf else {
                return;
            };
            let source = match slot {
                Slot::Read(buf) => program.reads[buf.0 as usize],
                Slot::Own => id,
            };
            let (reading, looped) = match (slot, at) {
                (_, At::Map(map)) if map.whole() => return,
                (Slot::Own, _) => ("loop", self.looped(id, bound).and_then(Result::ok)),
                (Slot::Read(_), At::Map(map)) if map.a == map.d => ("shift", None),
                (Slot::Read(_), At::Map(_)) => ("scale", None),
                (Slot::Read(_), At::Moving { .. }) => ("moving", None),
            };
            out.push(self.named(id, source, reading, bound, looped));
        });
    }

    /// Refuses, before a sample is computed, a loop no bound holds for or one past precision.
    pub(super) fn loops_bounded(&self) -> Result<(), EngineError> {
        let Some(bound) = kernel().bound(self.config.profile.ceiling_hz, self.lattice()) else {
            return Ok(());
        };
        for &id in &self.schedule.materialize {
            if let Some(looped) = self.looped(id, bound) {
                looped?;
            }
        }
        Ok(())
    }

    /// `None` where no kernel reads a linear loop's own past.
    fn looped(&self, id: NodeId, bound: Bound) -> Option<Result<f64, EngineError>> {
        let name = self.tys.name(id);
        let taps: Vec<(When, Option<Gain>)> = (0..self.tys.len())
            .map(|n| NodeId(n as u32))
            .filter(|n| self.tys.name(*n) == name)
            .filter_map(|n| match self.tys.value(n) {
                Value::SelfAt { at, gain } => Some((*at, *gain)),
                _ => None,
            })
            .collect();
        let read = |at: &When| match at {
            When::Time(back) => !loops::on_lattice(back.shift.neg()),
            When::Moving(_) => true,
        };
        if !taps.iter().any(|(at, _)| read(at)) {
            return None;
        }
        let gain = taps.iter().find_map(|(_, gain)| *gain)?;
        let extent = self.extents.of(id);
        let gap = || {
            taps.iter()
                .map(|(at, _)| self.gap(at, extent))
                .min()
                .expect("a loop reads a tap")
        };
        let loop_ = Loop {
            gain,
            fixed: taps.iter().all(|(at, _)| matches!(at, When::Time(_))),
            bound,
            generations: extent
                .is_bounded()
                .then(|| (extent.len() as f64 / gap() as f64).ceil()),
        };
        let (profile, lattice) = (&self.config.profile, f64::from(self.lattice()));
        let precision = profile.half_lsb();
        Some(match loop_.error() {
            Some(error) if error <= precision => Ok(error),
            Some(error) => Err(past_precision(
                name,
                error,
                profile.precision_bits,
                extent.is_bounded().then(|| extent.len() as f64 / lattice),
                loop_.passes(precision) * gap() as f64 / lattice,
            )),
            None => Err(unending(name, loop_.amplified())),
        })
    }

    /// Samples from the one a tap writes back to the latest its reading takes; one where no
    /// bound on the delay holds.
    fn gap(&self, at: &When, extent: Extent) -> i64 {
        let lattice = f64::from(self.lattice());
        let least = match at {
            When::Time(back) if loops::on_lattice(back.shift.neg()) => {
                return (back.shift.neg().to_f64() * lattice).round() as i64;
            }
            When::Time(back) => Some(back.shift.neg().to_f64()),
            When::Moving(time) => least_delay(&self.tys, *time, extent.start as f64 / lattice),
        };
        // Floored: a position's rounding stays under a sample.
        least.map_or(1, |d| {
            ((d * lattice).floor() as i64 - kernel().half_width() as i64).max(1)
        })
    }

    fn named(
        &self,
        node: NodeId,
        source: NodeId,
        reading: &'static str,
        bound: Bound,
        looped: Option<f64>,
    ) -> Reconstruction {
        Reconstruction {
            node: self.tys.name(node).to_string(),
            source: self.tys.name(source).to_string(),
            reading,
            bound,
            looped,
        }
    }
}

/// `t - time(t)` from `from` on, where `time` is `t` plus a sum each of whose atoms is bounded.
fn least_delay(tys: &Typing, time: NodeId, from: f64) -> Option<f64> {
    let sum = crate::refs::spectral_sum_of(tys, time, Var::T).ok()?;
    if !sum.lanes.iter().all(|lane| lane.is_finite_sum()) {
        return None;
    }
    let (mut line, mut least) = (false, 0.0);
    for atom in sum.atoms() {
        let plain = atom.c.im == 0.0
            && atom.exp.is_none()
            && atom.gauss.is_none()
            && atom.ind.is_none()
            && atom.pole.is_none()
            && !atom.is_delta();
        match (plain, atom.poly) {
            (true, 1) if atom.c.re == 1.0 && !line => line = true,
            (true, 0) => least -= atom.c.re,
            _ => least -= sup_from(atom, from)?,
        }
    }
    line.then_some(least)
}

/// Each kernel reading errs by `e` of full scale and amplifies an error in its samples at
/// most `L`-fold; `W`, `G` gain through whole and kernel taps. Each generation an error `x`
/// grows to `e G + a x`, `a = W + L G`: `e G (1 + ... + a^(K-1))` over `K`, `e G / (1 - a)`.
struct Loop {
    gain: Gain,
    fixed: bool,
    bound: Bound,
    generations: Option<f64>,
}

impl Loop {
    fn amplified(&self) -> f64 {
        self.gain.whole + self.bound.lebesgue * self.gain.kernel
    }

    fn injected(&self) -> f64 {
        self.bound.in_band * self.gain.kernel
    }

    fn error(&self) -> Option<f64> {
        let a = self.amplified();
        let horizon = self.generations.map(|k| self.injected() * geometric(a, k));
        let lasting = (a < 1.0).then(|| self.injected() / (1.0 - a));
        [
            spectral(self.fixed, self.gain.total(), self.bound),
            horizon,
            lasting,
        ]
        .into_iter()
        .flatten()
        .min_by(f64::total_cmp)
    }

    /// The most generations whose bound stays within `precision`.
    fn passes(&self, precision: f64) -> f64 {
        let (a, room) = (self.amplified(), precision / self.injected());
        let mut k = match a == 1.0 {
            true => room.floor(),
            false => ((1.0 + room * (a - 1.0)).ln() / a.ln()).floor().max(0.0),
        };
        while k > 0.0 && geometric(a, k) > room {
            k -= 1.0;
        }
        k
    }
}

fn geometric(a: f64, k: f64) -> f64 {
    match a == 1.0 {
        true => k,
        false => (a.powf(k) - 1.0) / (a - 1.0),
    }
}

/// A fixed-delay linear loop `Y = X / (1 - g A H)` against `X / (1 - g A)`: with `|H - 1| <= e`
/// and `|g A| <= G`, each component errs by at most `G e / (1 - G (1 + e))` of its own.
fn spectral(fixed: bool, gain: f64, bound: Bound) -> Option<f64> {
    let e = bound.in_band;
    let settles = fixed && gain * (1.0 + e) < 1.0;
    settles.then(|| gain * e / (1.0 - gain * (1.0 + e)))
}

fn past_precision(name: &str, error: f64, bits: i32, secs: Option<f64>, fits: f64) -> EngineError {
    let over = secs.map_or("with no end".to_string(), |s| format!("over its {s:.3} s"));
    EngineError::refused(Diagnostic {
        code: "engine.loop_error_past_precision".to_string(),
        message: format!(
            "this loop's readings between lattice samples may put its output off by {error:e} \
             of full scale {over}, past the profile's 2^-{bits}"
        ),
        location: Located::at(name, None),
        help: format!(
            "render at most {fits:.3} s of it, or lower its gain or lengthen its shortest delay"
        ),
    })
}

fn unending(name: &str, amplified: f64) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.loop_error_unbounded".to_string(),
        message: format!(
            "this loop has no end, and each pass through its readings between lattice samples \
             may grow an error {amplified}-fold, so no bound on its output holds"
        ),
        location: Located::at(name, None),
        help: "give it an end, or lower its gain until each pass shrinks an error".to_string(),
    })
}
