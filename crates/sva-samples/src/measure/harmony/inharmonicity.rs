// Concern: each moment's series: members' cents off n f1, a stiff-string B fit, energy in no series | Non-concern: naming the series (chords.rs) | IO: (moments, framing, floor) -> Inharmonicity

use super::chords::named;
use super::dissonance::Extreme;
use super::{Framing, Moment, Partial};

pub const MODEL: &str = "per moment, the series `pitch` names: each partial it names as a note \
                         roots one, and a partial it names the harmonic of a louder one joins \
                         that one's series, its harmonic number n = round(f / f1) over the root \
                         f1; only partials from 25 Hz to 5 kHz at or above the floor are read. \
                         That naming's tolerance, a ratio within 0.04 of a whole number from 1.5 \
                         to 8 over a louder partial, bounds how stretched a member may be, and a \
                         partial stretched past it, or a harmonic past the 8th with no louder \
                         partial at a divisor (a saw's 11th), roots a series of its own; a root \
                         is the partial named, so a series whose fundamental is quieter than its \
                         2nd harmonic splits in two; and a noise's many peaks fall within the \
                         tolerance by chance, so noise reads partly grouped, with many series. \
                         cents = 1200 log2(f / (n f1)); a series' and the moment's `cents` are \
                         the amplitude-weighted mean of |cents| over members n >= 2, null with \
                         none. fit: Fletcher, H. (1964), Normal vibration frequencies of a stiff \
                         piano string, J. Acoust. Soc. Am. 36(1) 203-209, f_n = n f0 sqrt(1 + B \
                         n^2), by unweighted least squares of (f_n / n)^2 = f0^2 + f0^2 B n^2 \
                         over a series' members, null under two distinct n or with no positive \
                         f0^2; B is the model's coefficient for a stiff string and only a fitted \
                         slope for any other body. unexplained: the share of the read partials' \
                         energy, sum of amplitude^2, in series of one partial, which hold no \
                         evidence of a harmonic series, so a lone sinusoid reads 1. Every bound \
                         holds over the box of the partials' frequency and amplitude bounds, \
                         with the series and each n as named at the estimates; b_bounds is null \
                         where that box admits no positive f0^2";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Member {
    pub n: u32,
    pub partial: Partial,
    pub cents: f64,
    pub cents_bounds: (f64, f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub b: f64,
    pub f0_hz: f64,
    /// Both `None` where the bounds' box admits a fit with no positive f0^2.
    pub b_bounds: Option<(f64, f64)>,
    pub f0_bounds: Option<(f64, f64)>,
}

/// `members` starts with the root, n = 1, then ascends by frequency.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub members: Vec<Member>,
    pub cents: Option<Weighted>,
    pub fit: Option<Fit>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weighted {
    pub value: f64,
    pub bounds: (f64, f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct InharmonicFrame {
    pub t_secs: f64,
    pub series: Vec<Series>,
    pub cents: Option<Weighted>,
    /// `None` where no partial is read.
    pub unexplained: Option<Weighted>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Inharmonicity {
    pub framing: Framing,
    pub floor_db: f64,
    pub frames: Vec<InharmonicFrame>,
    /// Over the frames where each is not null.
    pub cents_mean: Option<f64>,
    pub cents_max: Option<Extreme>,
    pub unexplained_mean: Option<f64>,
}

fn cents(ratio: f64) -> f64 {
    1200.0 * ratio.log2()
}

/// The root is n = 1 of itself, 0 cents whatever its bounds.
fn member(p: &Partial, n: u32, root: &Partial) -> Member {
    let k = f64::from(n);
    let cents_bounds = match n {
        1 => (0.0, 0.0),
        _ => (
            cents(p.hz_bounds.0 / (k * root.hz_bounds.1)),
            cents(p.hz_bounds.1 / (k * root.hz_bounds.0)),
        ),
    };
    Member {
        n,
        partial: *p,
        cents: cents(p.hz / (k * root.hz)),
        cents_bounds,
    }
}

/// One term of a weighted mean: its value and weight, each with its bounds.
#[derive(Clone, Copy)]
struct Term {
    value: f64,
    value_bounds: (f64, f64),
    weight: f64,
    weight_bounds: (f64, f64),
}

/// sum w v / sum w, and its bounds over every w and v in theirs: the ratio is linear
/// fractional in the weights, so its least value puts the high weight on every low value
/// under a threshold and the low weight above it (its greatest the reverse), and each split
/// of the values sorted is tried.
fn weighted(terms: &[Term]) -> Option<Weighted> {
    let weight: f64 = terms.iter().map(|t| t.weight).sum();
    if weight <= 0.0 {
        return None;
    }
    let value = terms.iter().map(|t| t.weight * t.value).sum::<f64>() / weight;
    let extreme = |least: bool| {
        let mut held: Vec<(f64, (f64, f64))> = terms
            .iter()
            .map(|t| match least {
                true => (t.value_bounds.0, t.weight_bounds),
                false => (-t.value_bounds.1, t.weight_bounds),
            })
            .collect();
        held.sort_by(|a, b| a.0.total_cmp(&b.0));
        let lowest = (0..=held.len())
            .filter_map(|split| {
                let (num, den) = held
                    .iter()
                    .enumerate()
                    .fold((0.0, 0.0), |(n, d), (i, (v, w))| {
                        let w = if i < split { w.1 } else { w.0 };
                        (n + w * v, d + w)
                    });
                (den > 0.0).then_some(num / den)
            })
            .fold(f64::INFINITY, f64::min);
        match least {
            true => lowest,
            false => -lowest,
        }
    };
    // The estimate lies in the box: taking it in absorbs summation-order rounding alone.
    Some(Weighted {
        value,
        bounds: (extreme(true).min(value), extreme(false).max(value)),
    })
}

/// |c| over the bounds of c.
fn magnitude(bounds: (f64, f64)) -> (f64, f64) {
    let (lo, hi) = bounds;
    let least = match lo <= 0.0 && hi >= 0.0 {
        true => 0.0,
        false => lo.abs().min(hi.abs()),
    };
    (least, lo.abs().max(hi.abs()))
}

/// Least squares of y = (f_n / n)^2 on n^2: its intercept a = f0^2 and slope f0^2 B are each
/// linear in y, so a's bounds over the box of y are a sum, and B's a linear-fractional program.
fn fit(points: &[(u32, Partial)]) -> Option<Fit> {
    let x: Vec<f64> = points.iter().map(|(n, _)| f64::from(*n).powi(2)).collect();
    let m = x.len() as f64;
    let mean = x.iter().sum::<f64>() / m;
    let sxx: f64 = x.iter().map(|x| (x - mean).powi(2)).sum();
    if sxx <= 0.0 {
        return None;
    }
    let slope: Vec<f64> = x.iter().map(|x| (x - mean) / sxx).collect();
    let intercept: Vec<f64> = slope.iter().map(|s| 1.0 / m - mean * s).collect();
    let y = |at: fn(&Partial) -> f64| -> Vec<f64> {
        points
            .iter()
            .map(|(n, p)| (at(p) / f64::from(*n)).powi(2))
            .collect()
    };
    let (y_lo, y_hi, y_at) = (y(|p| p.hz_bounds.0), y(|p| p.hz_bounds.1), y(|p| p.hz));
    let dot = |w: &[f64], y: &[f64]| w.iter().zip(y).map(|(w, y)| w * y).sum::<f64>();
    let a = dot(&intercept, &y_at);
    if a <= 0.0 {
        return None;
    }
    let b = dot(&slope, &y_at) / a;
    let a_end = |pick: fn(f64, f64) -> f64| -> f64 {
        intercept
            .iter()
            .zip(y_lo.iter().zip(&y_hi))
            .map(|(w, (lo, hi))| pick(w * lo, w * hi))
            .sum()
    };
    let (least_a, most_a) = (a_end(f64::min), a_end(f64::max));
    // Dinkelbach (1967): step to the vertex extremising sum (slope - l intercept) y.
    let ratio_bound = |sign: f64| {
        let mut l = b;
        loop {
            let y: Vec<f64> = slope
                .iter()
                .zip(&intercept)
                .enumerate()
                .map(|(i, (s, w))| match sign * (s - l * w) > 0.0 {
                    true => y_hi[i],
                    false => y_lo[i],
                })
                .collect();
            let next = dot(&slope, &y) / dot(&intercept, &y);
            if sign * (next - l) <= 0.0 {
                return l;
            }
            l = next;
        }
    };
    Some(Fit {
        b,
        f0_hz: a.sqrt(),
        b_bounds: (least_a > 0.0).then(|| (ratio_bound(-1.0), ratio_bound(1.0))),
        f0_bounds: (least_a > 0.0).then(|| (least_a.min(a).sqrt(), most_a.max(a).sqrt())),
    })
}

fn series_of(moment: &Moment, floor_db: f64) -> Vec<(Partial, Vec<(u32, Partial)>)> {
    let named = named(moment, floor_db);
    let mut roots: Vec<(Partial, Vec<(u32, Partial)>)> = Vec::new();
    let mut joined: Vec<usize> = Vec::with_capacity(named.len());
    for (partial, note) in &named {
        let at = match note.harmonic_of {
            None => {
                roots.push((*partial, vec![(1, *partial)]));
                roots.len() - 1
            }
            Some(hz) => {
                let louder = named
                    .iter()
                    .position(|(p, _)| p.hz == hz)
                    .expect("a harmonic names a louder named partial");
                let at = joined[louder];
                let n = (partial.hz / roots[at].0.hz).round() as u32;
                roots[at].1.push((n, *partial));
                at
            }
        };
        joined.push(at);
    }
    for (_, members) in &mut roots {
        members.sort_by(|a, b| a.1.hz.total_cmp(&b.1.hz));
    }
    roots
}

fn energy(p: &Partial) -> (f64, (f64, f64)) {
    (
        p.amplitude.powi(2),
        (p.amplitude_bounds.0.powi(2), p.amplitude_bounds.1.powi(2)),
    )
}

/// lone / (lone + grouped), rising in the first and falling in the second.
fn unexplained(lone: (f64, (f64, f64)), grouped: (f64, (f64, f64))) -> Option<Weighted> {
    let share = |u: f64, e: f64| match u + e > 0.0 {
        true => u / (u + e),
        false => 0.0,
    };
    (lone.0 + grouped.0 > 0.0).then(|| Weighted {
        value: share(lone.0, grouped.0),
        bounds: (share(lone.1.0, grouped.1.1), share(lone.1.1, grouped.1.0)),
    })
}

fn frame(moment: &Moment, floor_db: f64) -> InharmonicFrame {
    let add =
        |a: (f64, (f64, f64)), b: (f64, (f64, f64))| (a.0 + b.0, (a.1.0 + b.1.0, a.1.1 + b.1.1));
    let none = (0.0, (0.0, 0.0));
    let (mut lone, mut grouped) = (none, none);
    let mut every = Vec::new();
    let mut series = Vec::new();
    for (root, points) in series_of(moment, floor_db) {
        let held = points.iter().map(|(_, p)| energy(p)).fold(none, add);
        match points.len() {
            1 => lone = add(lone, held),
            _ => grouped = add(grouped, held),
        }
        let members: Vec<Member> = points.iter().map(|(n, p)| member(p, *n, &root)).collect();
        let deviations: Vec<Term> = members
            .iter()
            .zip(&points)
            .filter(|(m, _)| m.n >= 2)
            .map(|(m, (_, p))| Term {
                value: m.cents.abs(),
                value_bounds: magnitude(m.cents_bounds),
                weight: p.amplitude,
                weight_bounds: p.amplitude_bounds,
            })
            .collect();
        every.extend(&deviations);
        series.push(Series {
            cents: weighted(&deviations),
            fit: fit(&points),
            members,
        });
    }
    InharmonicFrame {
        t_secs: moment.t_secs,
        series,
        cents: weighted(&every),
        unexplained: unexplained(lone, grouped),
    }
}

pub fn measure(moments: &[Moment], framing: Framing, floor_db: f64) -> Inharmonicity {
    let frames: Vec<InharmonicFrame> = moments.iter().map(|m| frame(m, floor_db)).collect();
    let mean = |of: fn(&InharmonicFrame) -> Option<f64>| {
        let held: Vec<f64> = frames.iter().filter_map(of).collect();
        (!held.is_empty()).then(|| held.iter().sum::<f64>() / held.len() as f64)
    };
    let cents_max = frames
        .iter()
        .filter_map(|f| f.cents.map(|c| (c.value, f.t_secs)))
        .fold(None, |held: Option<Extreme>, (value, t_secs)| match held {
            Some(e) if e.value >= value => Some(e),
            _ => Some(Extreme { value, t_secs }),
        });
    Inharmonicity {
        framing,
        floor_db,
        cents_mean: mean(|f| f.cents.map(|c| c.value)),
        cents_max,
        unexplained_mean: mean(|f| f.unexplained.map(|u| u.value)),
        frames,
    }
}
