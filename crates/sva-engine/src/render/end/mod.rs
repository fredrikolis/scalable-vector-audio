// Concern: where an open render's root, or a stream's term heard at it, ends: silent where its bound is under the silence threshold | Non-concern: other nodes' zeros | IO: (node) -> support, cut

mod bound;

use std::rc::Rc;

use sva_formula::NodeId;
use sva_samples::{Extent, Grid, Profile};

use crate::render::value_graph::support::Supports;
use crate::typing::Typing;
use bound::{MagnitudeUpperBoundFromInstant, MagnitudeUpperBoundsFromInstant};

#[derive(Clone, Copy, Debug)]
pub(crate) struct End {
    pub(crate) support: Extent,
    pub(crate) treated_as_silent_from_sample: Option<i64>,
}

/// A stream's term as heard at its root.
pub(crate) struct Heard {
    pub(crate) support: Extent,
    pub(crate) fading: Option<Fading>,
}

#[derive(Clone)]
pub(crate) struct Fading {
    magnitude_upper_bound_from_instant: Rc<MagnitudeUpperBoundFromInstant>,
    grid: Grid,
    support: Extent,
}

impl Fading {
    pub(crate) fn from(&self, n: i64) -> f64 {
        match n >= self.support.end {
            true => 0.0,
            false => self
                .magnitude_upper_bound_from_instant
                .at_and_after_instant(self.grid.instant(n)),
        }
    }
}

/// The product's rounding included.
pub(crate) fn under(gain: f64, bound: f64, silence_threshold: f64) -> bool {
    let product = gain * bound;
    match gain == 1.0 {
        true => product < silence_threshold,
        false => product * (1.0 + 2.0 * f64::EPSILON) < silence_threshold,
    }
}

pub(crate) struct Ending<'a> {
    tys: &'a Typing,
    profile: &'a Profile,
    supports: &'a Supports<'a>,
    magnitude_upper_bounds_from_instant: MagnitudeUpperBoundsFromInstant,
}

impl<'a> Ending<'a> {
    pub(crate) fn new(tys: &'a Typing, profile: &'a Profile, supports: &'a Supports<'a>) -> Self {
        Ending {
            tys,
            profile,
            supports,
            magnitude_upper_bounds_from_instant: MagnitudeUpperBoundsFromInstant::default(),
        }
    }

    pub(crate) fn exact(&self, root: NodeId) -> End {
        End {
            support: self.supports.of(root),
            treated_as_silent_from_sample: None,
        }
    }

    /// The approved exception: zero from the first sample where the root's bound over every
    /// later instant is under the silence threshold.
    pub(crate) fn of(&self, root: NodeId) -> End {
        let uncut = self.exact(root);
        let treated_as_silent_from_sample = self
            .fading(root)
            .and_then(|f| self.first_sample_treated_as_silent(&f, 1.0));
        match treated_as_silent_from_sample {
            Some(at) => End {
                support: uncut.support.intersect(Extent::new(i64::MIN, at)),
                treated_as_silent_from_sample: Some(at),
            },
            None => uncut,
        }
    }

    pub(crate) fn gain(&self, reader: NodeId, read: NodeId) -> Option<f64> {
        bound::gain(self.tys, reader, read)
    }

    /// What `gain` of the same two is a function of.
    pub(crate) fn gain_key(&self, reader: NodeId, read: NodeId) -> Option<sva_formula::Hash> {
        bound::key(self.tys, reader, read)
    }

    pub(crate) fn heard(&self, term: NodeId, gain: Option<f64>) -> Heard {
        let support = self.supports.of(term);
        let fading = self.fading(term);
        let treated_as_silent_from_sample = fading
            .as_ref()
            .zip(gain)
            .and_then(|(f, g)| self.first_sample_treated_as_silent(f, g));
        Heard {
            support: treated_as_silent_from_sample
                .map_or(support, |at| support.intersect(Extent::new(i64::MIN, at))),
            fading,
        }
    }

    fn fading(&self, id: NodeId) -> Option<Fading> {
        let support = self.supports.of(id);
        if support.is_empty() || self.profile.silence_threshold_amplitude() <= 0.0 {
            return None;
        }
        let grid = self.tys.grid(id);
        let supported = |n: NodeId| self.supports.of(n);
        let rate = (self.profile, grid.rate);
        let magnitude_upper_bound_from_instant = MagnitudeUpperBoundFromInstant::of(
            self.tys,
            rate,
            id,
            &supported,
            &self.magnitude_upper_bounds_from_instant,
        )?;
        Some(Fading {
            magnitude_upper_bound_from_instant,
            grid,
            support,
        })
    }

    /// Where the bound falls monotonically.
    fn first_sample_treated_as_silent(&self, fading: &Fading, gain: f64) -> Option<i64> {
        let (exact, grid) = (fading.support, fading.grid);
        let silence_threshold = self.profile.silence_threshold_amplitude();
        let under = |n: i64| {
            gain == 0.0
                || under(
                    gain,
                    fading
                        .magnitude_upper_bound_from_instant
                        .at_and_after_instant(grid.instant(n)),
                    silence_threshold,
                )
        };
        let last = exact.end.saturating_sub(1);
        let from = exact.start.max(0).min(last);
        let probe = |k: i32| from.saturating_add(grid.count(2f64.powi(k)).ceil() as i64);
        let far = match exact.end {
            i64::MAX if under(probe(39)) => (0..40).map(probe).find(|n| under(*n)),
            i64::MAX => None,
            _ => under(last).then_some(last),
        }?;
        let (mut no, mut yes) = (from, far);
        if under(from) {
            yes = from;
        }
        while yes - no > 1 {
            let mid = no + (yes - no) / 2;
            match under(mid) {
                true => yes = mid,
                false => no = mid,
            }
        }
        Some(yes)
    }
}
