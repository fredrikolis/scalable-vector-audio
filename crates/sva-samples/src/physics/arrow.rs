// Concern: an arrow matrix of enclosed entries, proving one positive and bounding the ratio of two | Non-concern: which matrices a scheme makes (unison_tail.rs) | IO: (Arrow, Arrow) -> a proven bound

//! Positive exactly where every `a` and `corner - sum z^2/a` are. Each bound returned is proven
//! by that test over enclosures; the bisection only picks the candidate.

use crate::physics::ball::Ball;

#[derive(Clone)]
pub(crate) struct Arrow {
    pub(crate) diag: Vec<(Ball, Ball)>,
    pub(crate) corner: Ball,
}

impl Arrow {
    pub(crate) fn identity(modes: usize) -> Arrow {
        Arrow {
            diag: vec![(Ball::exact(1.0), Ball::exact(0.0)); modes],
            corner: Ball::exact(1.0),
        }
    }

    fn less(&self, c: f64, y: &Arrow) -> Arrow {
        Arrow {
            diag: self
                .diag
                .iter()
                .zip(&y.diag)
                .map(|(&(a, z), &(b, w))| (a.sub(b.scale(c)), z.sub(w.scale(c))))
                .collect(),
            corner: self.corner.sub(y.corner.scale(c)),
        }
    }

    fn scaled(&self, c: f64) -> Arrow {
        Arrow {
            diag: self
                .diag
                .iter()
                .map(|&(a, z)| (a.scale(c), z.scale(c)))
                .collect(),
            corner: self.corner.scale(c),
        }
    }

    pub(crate) fn schur(&self) -> Option<Ball> {
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

    fn seems(x: &Arrow, p: f64, y: &Arrow, q: f64) -> bool {
        let mut pulled = 0.0;
        for (&(a, z), &(b, w)) in x.diag.iter().zip(&y.diag) {
            let d = p * a.c - q * b.c;
            if d <= 0.0 {
                return false;
            }
            let t = p * z.c - q * w.c;
            pulled += t * t / d;
        }
        p * x.corner.c - q * y.corner.c - pulled > 0.0
    }

    /// `x'A^-1 x`, `x` given as `(index, x_m, sign of z_m)` with no corner part.
    pub(crate) fn inverse_at(&self, parts: &[(usize, Ball, Ball)]) -> Option<Ball> {
        let (mut own, mut through) = (Ball::exact(0.0), Ball::exact(0.0));
        for &(m, x, sign) in parts {
            let (a, z) = self.diag[m];
            own = own.add(x.square().div(a)?);
            through = through.add(z.mul(sign).mul(x).div(a)?);
        }
        Some(own.add(through.square().div(self.schur()?)?))
    }
}

fn search(
    mut fails: f64,
    mut holds: f64,
    seems: &dyn Fn(f64) -> bool,
    proven: &dyn Fn(f64) -> bool,
) -> Option<f64> {
    for _ in 0..200 {
        let mid = fails + (holds - fails) / 2.0;
        if mid == fails || mid == holds {
            break;
        }
        match seems(mid) {
            true => holds = mid,
            false => fails = mid,
        }
    }
    let toward = holds - fails;
    let mut out = 1e-12 * holds.abs().max(toward.abs());
    for _ in 0..48 {
        let c = holds + out.copysign(toward);
        if proven(c) {
            return Some(c);
        }
        out *= 4.0;
    }
    None
}

/// A proven `c` with `c y - x > 0`: over every `x'Xx/x'Yx`, `y` positive.
pub(crate) fn ceiling(x: &Arrow, y: &Arrow) -> Option<f64> {
    let over = |c: f64| y.scaled(c).less(1.0, x);
    let seems = |c: f64| Arrow::seems(y, c, x, 1.0);
    let mut hi = 1.0f64;
    while !seems(hi) {
        hi *= 2.0;
        (hi < 1e300).then_some(())?;
    }
    search(0.0, hi, &seems, &|c| over(c).proven_positive())
}

/// A proven `c >= 0` with `x - c y > 0`: under every `x'Xx/x'Yx`.
pub(crate) fn floor(x: &Arrow, y: &Arrow) -> Option<f64> {
    x.proven_positive().then_some(())?;
    let under = |c: f64| x.less(c, y);
    let seems = |c: f64| Arrow::seems(x, 1.0, y, c);
    let mut lo = 1.0f64;
    while seems(lo) {
        lo *= 2.0;
        (lo < 1e300).then_some(())?;
    }
    Some(search(lo, 0.0, &seems, &|c| c > 0.0 && under(c).proven_positive()).unwrap_or(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrow(diag: &[(f64, f64)], corner: f64) -> Arrow {
        Arrow {
            diag: diag
                .iter()
                .map(|&(a, z)| (Ball::exact(a), Ball::exact(z)))
                .collect(),
            corner: Ball::exact(corner),
        }
    }

    /// `[[a, z], [z, d]]` over the identity: its eigenvalues in closed form.
    #[test]
    fn the_ratio_bounds_bracket_the_eigenvalues_they_certify() {
        for (a, z, d) in [(2.0, 0.5, 1.0), (1e-4, 3e-3, 1.0), (4.0, 0.0, 1e-6)] {
            let x = arrow(&[(a, z)], d);
            let id = Arrow::identity(1);
            let (mid, half) = ((a + d) / 2.0, (((a - d) / 2.0).powi(2) + z * z).sqrt());
            let (least, most) = (mid - half, mid + half);
            let top = ceiling(&x, &id).expect("a ceiling");
            assert!(
                top >= most && top <= most * (1.0 + 1e-9),
                "{top} over {most}"
            );
            let low = floor(&x, &id).expect("a positive arrow");
            assert!(
                low <= least && low >= least * (1.0 - 1e-6),
                "{low} under {least}"
            );
        }
    }
}
