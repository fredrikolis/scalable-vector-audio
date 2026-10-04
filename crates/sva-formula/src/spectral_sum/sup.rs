// Concern: bounds one atom's magnitude from an instant on, and whether it ever decays | Non-concern: summing atoms, deciding silence | IO: (&SpectralAtom, t) -> f64 or none

use super::atom::{Singular, SpectralAtom};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    Decays,
    /// A line or a constant: `|c|` forever.
    Steady,
    Grows,
}

/// `sup |atom(s)|` over `s >= t`, `None` for a delta or a pole on the interval. The power
/// and the exponential peak together at an end or at their one stationary point.
pub fn magnitude_upper_bound_from_instant(atom: &SpectralAtom, t: f64) -> Option<f64> {
    if let Singular::Delta { .. } = atom.sing {
        return None;
    }
    let (lo, hi) = match atom.ind {
        Some(ind) => (t.max(ind.l.value()), ind.r.value()),
        None => (t, f64::INFINITY),
    };
    if lo >= hi || atom.c.abs() == 0.0 {
        return Some(0.0);
    }
    let (sigma, mu) = atom.exp.map_or((0.0, 0.0), |e| (e.sigma, e.mu));
    let (p, from) = (f64::from(atom.poly.degree), atom.poly.at);
    let ln = |s: f64| {
        let grown = match sigma == 0.0 {
            true => 0.0,
            false => sigma * (s - mu),
        };
        let power = match (p == 0.0, s == from) {
            (true, _) => 0.0,
            (false, true) => f64::NEG_INFINITY,
            (false, false) => p * (s - from).abs().ln(),
        };
        match grown.is_infinite() {
            true => grown,
            false => power + grown,
        }
    };
    let mut top = ln(lo).max(ln(hi));
    if p > 0.0 && sigma != 0.0 {
        let stationary = from - p / sigma;
        if stationary >= lo && stationary < hi {
            top = top.max(ln(stationary));
        }
    }
    let gauss = atom.gauss.map_or(0.0, |g| {
        let d = (lo - g.mu).max(0.0).max(g.mu - hi);
        -g.a * d * d
    });
    let (pole, pole_ln) = match atom.pole {
        None => (1.0, 0.0),
        Some(pole) => {
            let dx = (lo - pole.at.re).max(0.0).max(pole.at.re - hi);
            let d = dx.hypot(pole.at.im);
            if d == 0.0 {
                return None;
            }
            let order = i32::from(pole.order);
            (d.powi(-order), -f64::from(order) * d.ln())
        }
    };
    let factors = [top.exp(), gauss.exp(), pole];
    if factors.iter().all(|f| f.is_normal()) {
        return Some(atom.c.abs() * factors[0] * factors[1] * factors[2]);
    }
    // A factor past a double's range: their logs sum, each sum and the exp off by an ulp each.
    let terms = [top, gauss, pole_ln];
    let ln = terms.iter().sum::<f64>();
    if ln == f64::NEG_INFINITY {
        return Some(0.0);
    }
    let slack = 1.0 + 4.0 * f64::EPSILON * (1.0 + terms.iter().map(|x| x.abs()).sum::<f64>());
    Some(atom.c.abs() * ln.exp() * slack).filter(|s| !s.is_nan())
}

pub fn fate(atom: &SpectralAtom) -> Fate {
    let ends = atom.ind.is_some_and(|i| i.r.value().is_finite());
    if ends || atom.gauss.is_some() {
        return Fate::Decays;
    }
    let sigma = atom.exp.map_or(0.0, |e| e.sigma);
    let power = i32::from(atom.poly.degree) - atom.pole.map_or(0, |p| i32::from(p.order));
    match (sigma.total_cmp(&0.0), power.cmp(&0)) {
        (std::cmp::Ordering::Less, _) => Fate::Decays,
        (std::cmp::Ordering::Greater, _) => Fate::Grows,
        (_, std::cmp::Ordering::Less) => Fate::Decays,
        (_, std::cmp::Ordering::Equal) if atom.pole.is_none() => Fate::Steady,
        (_, std::cmp::Ordering::Equal) => Fate::Decays,
        (_, std::cmp::Ordering::Greater) => Fate::Grows,
    }
}
