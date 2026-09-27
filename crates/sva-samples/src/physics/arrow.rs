// Concern: an arrow matrix of enclosed entries, proving it positive | Non-concern: which matrices a scheme makes (unison_energy.rs) | IO: (Arrow) -> proven or not

//! Positive exactly where every `a` and `corner - sum z^2/a` are, tested over enclosures.

use crate::physics::ball::Ball;

pub(crate) struct Arrow {
    pub(crate) diag: Vec<(Ball, Ball)>,
    pub(crate) corner: Ball,
}

impl Arrow {
    fn schur(&self) -> Option<Ball> {
        let mut s = self.corner;
        for &(a, z) in &self.diag {
            (a.lo() > 0.0).then_some(())?;
            s = s.sub(z.square().div(a)?);
        }
        Some(s)
    }

    pub(crate) fn proven_positive(&self) -> bool {
        self.schur().is_some_and(|s| s.lo() > 0.0)
    }
}
