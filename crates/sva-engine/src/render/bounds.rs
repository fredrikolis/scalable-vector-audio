// Concern: each reading between lattice samples, the kernel it takes and its bound, refused past precision | Non-concern: one reading's proof (sva-samples) | IO: (&Render) -> bounds or a refusal

use sva_formula::spectral_sum::sup::sup_from;
use sva_formula::{Held, NodeId, Var};
use sva_samples::machine::position_error;
use sva_samples::{At, Bound, Extent, NodeRenderer, PSYCHOACOUSTIC_V1, Slot, kernel, plain};

use super::{Render, extent, pointwise, sampled};
use crate::error::{Diagnostic, EngineError, Located};
use crate::loops;
use crate::recirculation::Loop;
use crate::typing::{Gain, Typing, Value, When};

#[derive(Clone, Debug, PartialEq)]
pub struct Reconstruction {
    pub node: String,
    pub source: String,
    /// `shift`, `scale`, `moving`, `loop`, `point`, `constant` or `output`.
    pub reading: &'static str,
    pub bound: Bound,
    /// A loop's proven bound on its own output, relative to its full scale.
    pub looped: Option<f64>,
}

struct Row {
    source: NodeId,
    reading: &'static str,
    bound: Option<Bound>,
    looped: Option<f64>,
}

/// The kernel a loop reads its own past through and the proven error of its output.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Looped {
    pub half_width: usize,
    pub error: f64,
    pub bound: Bound,
}

impl Render {
    pub fn reconstructions(&self) -> Vec<Reconstruction> {
        let Some(bound) = plain_bound() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &id in &self.schedule.materialize {
            match self.tys.ty(id).held {
                Held::Sampled => self.programmed(id, &mut out),
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
        if let Some(output) = self.output
            && let Ok(Some(program)) = sampled::at_output(self, self.root, &|_| 1)
        {
            for row in self.rows(self.root, &program, (output, self.config.rate)) {
                if let Row { bound: Some(b), .. } = row {
                    out.push(self.named(self.root, row.source, "output", b, None));
                }
            }
        } else if self.output.is_some() && !self.out_map().whole() {
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

    fn programmed(&self, id: NodeId, out: &mut Vec<Reconstruction>) {
        let Ok(program) = sampled::program(self, id) else {
            return;
        };
        for row in self.rows(id, &program, (self.extents.of(id), self.lattice())) {
            if let Row {
                bound: Some(bound), ..
            } = row
            {
                out.push(self.named(id, row.source, row.reading, bound, row.looped));
            }
        }
    }

    /// Each read of `program` between its source's samples over `extent` of instants `rate` a
    /// second apart; `bound` is `None` where no bound on its position holds.
    fn rows(&self, id: NodeId, program: &sampled::Program, over: (Extent, u32)) -> Vec<Row> {
        let mut out = Vec::new();
        extent::leaves(&program.renderer, &mut |leaf| {
            let NodeRenderer::Read { slot, at, .. } = leaf else {
                return;
            };
            let source = match slot {
                Slot::Read(buf) => program.reads[buf.0 as usize],
                Slot::Own => id,
            };
            let (reading, bound, looped) = match (slot, at) {
                (_, At::Map(map)) if map.whole() => return,
                (Slot::Own, _) => match self.loop_kernel(id, over.0) {
                    Ok(Some(held)) => ("loop", Some(held.bound), Some(held.error)),
                    _ => return,
                },
                (Slot::Read(_), At::Map(map)) if map.a == map.d => ("shift", plain_bound(), None),
                (Slot::Read(_), At::Map(_)) => ("scale", plain_bound(), None),
                (Slot::Read(_), At::Moving { .. }) => ("moving", self.moved(at, over), None),
            };
            out.push(Row {
                source,
                reading,
                bound,
                looped,
            });
        });
        out
    }

    /// Refuses, before a sample is computed, a reading or a loop no bound within precision holds for.
    pub(super) fn readings_bounded(&self) -> Result<(), EngineError> {
        for &id in &self.schedule.materialize {
            if self.tys.ty(id).held != Held::Sampled {
                continue;
            }
            let extent = self.extents.of(id);
            self.loop_kernel(id, extent)?;
            let Ok(program) = sampled::program(self, id) else {
                continue;
            };
            self.bounded(id, &self.rows(id, &program, (extent, self.lattice())))?;
        }
        if let (Some(output), Some(program)) =
            (self.output, sampled::at_output(self, self.root, &|_| 1)?)
        {
            self.bounded(
                self.root,
                &self.rows(self.root, &program, (output, self.config.rate)),
            )?;
        }
        Ok(())
    }

    fn bounded(&self, id: NodeId, rows: &[Row]) -> Result<(), EngineError> {
        let precision = self.config.profile.half_lsb();
        for row in rows.iter().filter(|row| row.reading == "moving") {
            match row.bound {
                None => return Err(unplaced(self.tys.name(id))),
                Some(b) if b.in_band > precision => {
                    return Err(placed_past(self.tys.name(id), b, precision));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// The plain kernel's bound, with a moving position's rounding over `extent` counted.
    fn moved(&self, at: &At, (extent, rate): (Extent, u32)) -> Option<Bound> {
        let delta = position_error(at, (extent.start, extent.end), rate)?;
        Some(plain_bound()?.moved(delta, self.lattice()))
    }

    /// `None` where no tap reads the loop's own past between lattice samples.
    pub(crate) fn loop_kernel(
        &self,
        id: NodeId,
        extent: Extent,
    ) -> Result<Option<Looped>, EngineError> {
        let name = self.tys.name(id);
        let taps: Vec<(When, Option<Gain>)> = (0..self.tys.len())
            .map(|n| NodeId(n as u32))
            .filter(|n| self.tys.name(*n) == name)
            .filter_map(|n| match self.tys.value(n) {
                Value::SelfAt { at, gain } => Some((*at, *gain)),
                _ => None,
            })
            .collect();
        let kernel_read = |at: &When| match at {
            When::Time(back) => !loops::on_lattice(back.shift.neg()),
            When::Moving(_) => true,
        };
        if !taps.iter().any(|(at, _)| kernel_read(at)) {
            return Ok(None);
        }
        let Some(gain) = taps.iter().find_map(|(_, gain)| *gain) else {
            return Err(unproven(name));
        };
        let lattice = self.lattice();
        let mut held = Loop::over(gain, extent.is_bounded().then(|| extent.len() as i64));
        for (at, _) in &taps {
            match at {
                When::Time(back) => held.back(back.shift.neg()),
                When::Moving(time) => {
                    let at = sampled::closed_renderer(&self.tys, *time)
                        .map(|r| At::moving(f64::from(lattice), f64::from(lattice), r, true));
                    let Some(delta) =
                        at.and_then(|at| position_error(&at, (extent.start, extent.end), lattice))
                    else {
                        return Err(unplaced(name));
                    };
                    let from = extent.start as f64 / f64::from(lattice);
                    let least = least_delay(&self.tys, *time, from)
                        .map_or(1, |d| (d * f64::from(lattice) - delta).ceil() as i64);
                    held.moving(least, delta);
                }
            }
        }
        match held.kernel() {
            Some((k, error)) => Ok(Some(Looped {
                half_width: k.half_width(),
                error,
                bound: held.bound(k).expect("a kernel the loop took has a bound"),
            })),
            None => Err(past(name, &held, extent)),
        }
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

fn plain_bound() -> Option<Bound> {
    let p = PSYCHOACOUSTIC_V1;
    plain().bound(p.ceiling_hz, p.lattice_hz)
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

fn refusal(code: &str, name: &str, message: String, help: String) -> EngineError {
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message,
        location: Located::at(name, None),
        help,
    })
}

/// The widest kernel a loop's delay leaves room for says how far it runs within precision.
fn past(name: &str, held: &Loop, extent: Extent) -> EngineError {
    let family = PSYCHOACOUSTIC_V1.kernel;
    let Some(widest) = family.lengths(held.most()).last().map(kernel) else {
        return refusal(
            "engine.loop_reads_ahead",
            name,
            format!(
                "this loop reads its own past {} samples back, between two samples, and the \
                 shortest kernel takes {} samples either side, some not yet written",
                held.least_delay, family.step
            ),
            format!(
                "write a delay of more than {} samples, or a whole number of sp",
                family.step
            ),
        );
    };
    let bits = PSYCHOACOUSTIC_V1.precision_bits;
    let lattice = f64::from(PSYCHOACOUSTIC_V1.lattice_hz);
    let error = held.bound(widest).and_then(|b| held.error(&b));
    match (error, extent.is_bounded()) {
        (None, false) => refusal(
            "engine.loop_error_unbounded",
            name,
            "this loop has no end, and each pass through its readings between lattice samples \
             may grow an error, so no bound on its output holds"
                .to_string(),
            "give it an end, or lower its gain until each pass shrinks an error".to_string(),
        ),
        (error, bounded) => {
            let over = match bounded {
                true => format!("over its {:.3} s", extent.len() as f64 / lattice),
                false => "with no end".to_string(),
            };
            let fits = held.fits(widest).unwrap_or(0.0) / lattice;
            refusal(
                "engine.loop_error_past_precision",
                name,
                format!(
                    "this loop's readings between lattice samples may put its output off by \
                     {} of full scale {over} through its widest kernel, {} taps, past the \
                     profile's 2^-{bits}",
                    error.map_or("an unbounded amount".to_string(), |e| format!("{e:e}")),
                    2 * widest.half_width()
                ),
                format!(
                    "render at most {fits:.3} s of it, or lower its gain or lengthen its \
                     shortest delay"
                ),
            )
        }
    }
}

fn unproven(name: &str) -> EngineError {
    refusal(
        "engine.loop_error_unproven",
        name,
        "this loop reads its own past between lattice samples through an operation no bound on \
         the error it passes on is known for"
            .to_string(),
        "write the loop from gains, fixed filters, crops, tanh, sat, sin, cos, abs, min and \
         max, or read its past at whole samples"
            .to_string(),
    )
}

fn unplaced(name: &str) -> EngineError {
    refusal(
        "engine.position_unbounded",
        name,
        "a read here moves through a time whose computed value no rounding bound holds for over \
         its extent, so where it reads is not known closely enough"
            .to_string(),
        "write the time as a closed form of t over a bounded extent, away from a jump any \
         sample could round across"
            .to_string(),
    )
}

fn placed_past(name: &str, bound: Bound, precision: f64) -> EngineError {
    refusal(
        "engine.reading_past_precision",
        name,
        format!(
            "a moving read here may err by {:e} of each component, {} samples of it its \
             position's rounding, past the profile's {precision:e}",
            bound.in_band, bound.position
        ),
        "read over a shorter extent, or write the time as t less a delay".to_string(),
    )
}
