// Concern: where an open render's root, or a stream's term heard at it, ends: cut where its bound there is under the level | Non-concern: where other nodes are nonzero | IO: (node) -> support, cut

mod bound;

use std::rc::Rc;

use sva_formula::NodeId;
use sva_samples::{Extent, Grid, Profile};

use crate::render::table::support::Supports;
use crate::typing::Typing;
use bound::{Tail, Tails};

#[derive(Clone, Copy, Debug)]
pub(crate) struct End {
    pub(crate) support: Extent,
    pub(crate) cut: Option<i64>,
}

/// A stream's term as heard at its root.
pub(crate) struct Heard {
    pub(crate) support: Extent,
    pub(crate) fading: Option<Fading>,
}

#[derive(Clone)]
pub(crate) struct Fading {
    tail: Rc<Tail>,
    grid: Grid,
    support: Extent,
}

impl Fading {
    pub(crate) fn from(&self, n: i64) -> f64 {
        match n >= self.support.end {
            true => 0.0,
            false => self.tail.from(self.grid.instant(n)),
        }
    }
}

/// The product's rounding included.
pub(crate) fn under(gain: f64, bound: f64, level: f64) -> bool {
    let product = gain * bound;
    match gain == 1.0 {
        true => product < level,
        false => product * (1.0 + 2.0 * f64::EPSILON) < level,
    }
}

pub(crate) struct Ending<'a> {
    tys: &'a Typing,
    profile: &'a Profile,
    supports: &'a Supports<'a>,
    tails: Tails,
}

impl<'a> Ending<'a> {
    pub(crate) fn new(tys: &'a Typing, profile: &'a Profile, supports: &'a Supports<'a>) -> Self {
        Ending {
            tys,
            profile,
            supports,
            tails: Tails::default(),
        }
    }

    pub(crate) fn exact(&self, root: NodeId) -> End {
        End {
            support: self.supports.of(root),
            cut: None,
        }
    }

    /// The approved exception: zero from the first sample where the root's bound over every
    /// later instant is under the level.
    pub(crate) fn of(&self, root: NodeId) -> End {
        let uncut = self.exact(root);
        let quiet = self.fading(root).and_then(|f| self.quiet(&f, 1.0));
        match quiet {
            Some(at) => End {
                support: uncut.support.intersect(Extent::new(i64::MIN, at)),
                cut: Some(at),
            },
            None => uncut,
        }
    }

    pub(crate) fn gain(&self, reader: NodeId, read: NodeId) -> Option<f64> {
        bound::gain(self.tys, reader, read)
    }

    pub(crate) fn heard(&self, term: NodeId, gain: Option<f64>) -> Heard {
        let support = self.supports.of(term);
        let fading = self.fading(term);
        let quiet = fading
            .as_ref()
            .zip(gain)
            .and_then(|(f, g)| self.quiet(f, g));
        Heard {
            support: quiet.map_or(support, |at| support.intersect(Extent::new(i64::MIN, at))),
            fading,
        }
    }

    fn fading(&self, id: NodeId) -> Option<Fading> {
        let support = self.supports.of(id);
        if support.is_empty() || self.profile.prune_level() <= 0.0 {
            return None;
        }
        let grid = self.tys.grid(id);
        let supported = |n: NodeId| self.supports.of(n);
        let rate = (self.profile, grid.rate);
        let tail = Tail::of(self.tys, rate, id, &supported, &self.tails)?;
        Some(Fading {
            tail,
            grid,
            support,
        })
    }

    /// Where the bound falls monotonically.
    fn quiet(&self, fading: &Fading, gain: f64) -> Option<i64> {
        let (exact, grid) = (fading.support, fading.grid);
        let level = self.profile.prune_level();
        let under = |n: i64| gain == 0.0 || under(gain, fading.tail.from(grid.instant(n)), level);
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
