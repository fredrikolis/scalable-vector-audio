// Concern: gathers lines on one ladder into runs, each summed as a unit | Non-concern: evaluating a run or bounding its rounding (sva-samples) | IO: (&[Line]) -> Vec<Run>

use std::collections::BTreeMap;

use crate::complex::{C64, canonical};
use crate::series::{Line, Rung};

/// Lines at exactly `offset + step*k` Hz for `k` in `first..first + amps.len()`, and where
/// `mirror` holds one, the same ladder negated.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub offset: f64,
    pub step: f64,
    pub first: i64,
    pub amps: Vec<C64>,
    pub mirror: Mirror,
}

/// `Conjugate` where each amplitude is bit for bit its partner's conjugate.
#[derive(Clone, Debug, PartialEq)]
pub enum Mirror {
    None,
    Conjugate,
    Held(Vec<C64>),
}

impl Run {
    /// Loose lines are runs of one; a ladder breaks wherever its index skips.
    pub fn of(lines: &[Line]) -> Vec<Run> {
        let mut runs: Vec<Run> = Vec::new();
        let mut open: BTreeMap<(u64, u64), usize> = BTreeMap::new();
        for line in lines {
            let rung = line.rung.unwrap_or(Rung {
                offset: line.hz,
                step: 0.0,
                k: 0,
            });
            let key = (canonical(rung.offset), canonical(rung.step));
            match open.get(&key) {
                Some(&at) if rung.step != 0.0 && runs[at].next() == Some(rung.k) => {
                    runs[at].amps.push(line.amp);
                }
                _ => {
                    open.insert(key, runs.len());
                    runs.push(Run {
                        offset: rung.offset,
                        step: rung.step,
                        first: rung.k,
                        amps: vec![line.amp],
                        mirror: Mirror::None,
                    });
                }
            }
        }
        paired(runs)
    }

    /// Lines held, the mirror's included.
    pub fn len(&self) -> usize {
        match &self.mirror {
            Mirror::None => self.amps.len(),
            Mirror::Conjugate => 2 * self.amps.len(),
            Mirror::Held(amps) => self.amps.len() + amps.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.amps.is_empty()
    }

    pub fn mirrored(&self) -> Option<Vec<C64>> {
        match &self.mirror {
            Mirror::None => None,
            Mirror::Conjugate => Some(self.amps.iter().map(|a| a.conj()).collect()),
            Mirror::Held(amps) => Some(amps.clone()),
        }
    }

    pub fn lines(&self) -> Vec<Line> {
        let at = |i: usize, sign: f64, amp: C64| {
            let k = self.first + i as i64;
            let rung = Rung {
                offset: sign * self.offset,
                step: sign * self.step,
                k,
            };
            Line {
                hz: rung.offset + rung.step * k as f64,
                amp,
                rung: Some(rung),
            }
        };
        let mut out: Vec<Line> = self
            .amps
            .iter()
            .enumerate()
            .map(|(i, a)| at(i, 1.0, *a))
            .collect();
        if let Some(mirror) = self.mirrored() {
            out.extend(mirror.into_iter().enumerate().map(|(i, a)| at(i, -1.0, a)));
        }
        out
    }

    fn next(&self) -> Option<i64> {
        self.first.checked_add(self.amps.len() as i64)
    }

    fn key(&self, sign: f64) -> (u64, u64, i64, usize) {
        let (offset, step) = (sign * self.offset, sign * self.step);
        (
            canonical(offset),
            canonical(step),
            self.first,
            self.amps.len(),
        )
    }
}

/// Each run folds in the first later one on its negated ladder over the same indices.
fn paired(runs: Vec<Run>) -> Vec<Run> {
    let mut waiting: BTreeMap<(u64, u64, i64, usize), Vec<usize>> = BTreeMap::new();
    for (at, run) in runs.iter().enumerate().rev() {
        waiting.entry(run.key(1.0)).or_default().push(at);
    }
    let mut slots: Vec<Option<Run>> = runs.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(slots.len());
    for at in 0..slots.len() {
        let Some(mut run) = slots[at].take() else {
            continue;
        };
        let partner = waiting.get_mut(&run.key(-1.0)).and_then(|queue| {
            while let Some(&next) = queue.last() {
                match next > at && slots[next].is_some() {
                    true => return Some(next),
                    false => queue.pop(),
                };
            }
            None
        });
        if let Some(other) = partner.and_then(|found| slots[found].take()) {
            let pairs = run.amps.iter().zip(&other.amps);
            run.mirror = match pairs.clone().all(|(a, b)| conjugate(*a, *b)) {
                true => Mirror::Conjugate,
                false => Mirror::Held(other.amps),
            };
        }
        out.push(run);
    }
    out
}

fn conjugate(a: C64, b: C64) -> bool {
    a.re.to_bits() == b.re.to_bits() && (-a.im).to_bits() == b.im.to_bits()
}
