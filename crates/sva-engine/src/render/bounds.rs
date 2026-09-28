// Concern: names each reading a render takes between lattice samples and the bound it carries | Non-concern: the bound's proof (sva-samples reconstruct) | IO: (&Render) -> Vec<Reconstruction>

use sva_formula::{Held, NodeId};
use sva_samples::{At, Bound, NodeRenderer, Slot, kernel};

use super::{Render, extent, pointwise, sampled};
use crate::typing::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Reconstruction {
    pub node: String,
    pub source: String,
    /// `shift`, `scale`, `moving`, `loop`, `point`, `constant` or `output`.
    pub reading: &'static str,
    pub bound: Bound,
    /// A fixed-delay linear loop's in-band bound on its own output, per component.
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
        let gain = self.loop_gain(id);
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
                (Slot::Own, At::Map(_)) => ("loop", gain.and_then(|g| amplified(g, bound))),
                (Slot::Own, At::Moving { .. }) => ("loop", None),
                (Slot::Read(_), At::Map(map)) if map.a == map.d => ("shift", None),
                (Slot::Read(_), At::Map(_)) => ("scale", None),
                (Slot::Read(_), At::Moving { .. }) => ("moving", None),
            };
            out.push(self.named(id, source, reading, bound, looped));
        });
    }

    fn loop_gain(&self, id: NodeId) -> Option<f64> {
        let name = self.tys.name(id);
        (0..self.tys.len())
            .map(|n| NodeId(n as u32))
            .filter(|n| self.tys.name(*n) == name)
            .find_map(|n| match self.tys.value(n) {
                Value::SelfAt { gain, .. } => *gain,
                _ => None,
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

/// A fixed-delay linear loop `Y = X / (1 - g A H)` against `X / (1 - g A)`: with `|H - 1| <= e`
/// and `|g A| <= G`, each component errs by at most `G e / (1 - G (1 + e))` of its own.
fn amplified(gain: f64, bound: Bound) -> Option<f64> {
    let e = bound.in_band;
    let settles = gain * (1.0 + e) < 1.0;
    settles.then(|| gain * e / (1.0 - gain * (1.0 + e)))
}
