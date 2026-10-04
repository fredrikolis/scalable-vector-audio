// Concern: where an open render's sound ends: its root's support, cut where the root's bound falls under the level | Non-concern: where other nodes are nonzero | IO: (root) -> support, cut

mod bound;

use sva_formula::NodeId;
use sva_samples::{Extent, Profile};

use crate::render::table::support::Supports;
use crate::typing::Typing;
use bound::{Tail, Tails};

#[derive(Clone, Copy, Debug)]
pub(crate) struct End {
    pub(crate) support: Extent,
    pub(crate) cut: Option<i64>,
}

/// Bounds found once across the roots asked.
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

    /// The approved exception to exact supports: zero from the first sample where the root's
    /// bound over every later instant is under the level, where that bound falls monotonically.
    pub(crate) fn exact(&self, root: NodeId) -> End {
        End {
            support: self.supports.of(root),
            cut: None,
        }
    }

    pub(crate) fn of(&self, root: NodeId) -> End {
        let uncut = self.exact(root);
        let exact = uncut.support;
        let level = self.profile.prune_level();
        if exact.is_empty() || level <= 0.0 {
            return uncut;
        }
        let grid = self.tys.grid(root);
        let supported = |n: NodeId| self.supports.of(n);
        let rate = (self.profile, grid.rate);
        let Some(tail) = Tail::of(self.tys, rate, root, &supported, &self.tails) else {
            return uncut;
        };
        let under = |n: i64| tail.from(grid.instant(n)) < level;
        let last = exact.end.saturating_sub(1);
        let from = exact.start.max(0).min(last);
        let probe = |k: i32| from.saturating_add(grid.count(2f64.powi(k)).ceil() as i64);
        let far = match exact.end {
            i64::MAX if under(probe(39)) => (0..40).map(probe).find(|n| under(*n)),
            i64::MAX => None,
            _ => under(last).then_some(last),
        };
        let Some(far) = far else {
            return uncut;
        };
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
        let support = match exact.start < yes {
            true => Extent::new(exact.start, yes),
            false => Extent::NOWHERE,
        };
        End {
            support,
            cut: Some(yes),
        }
    }
}
