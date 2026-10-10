// Concern: Sethares' (1993) Plomp-Levelt roughness per moment, and the curve's extremes | Non-concern: where the partials came from | IO: (moments, framing, floor) -> Dissonance or Crowded

use super::{Framing, Moment, Partial};

pub const MODEL: &str = "Sethares, W. A. (1993), Local consonance and the relationship \
                         between timbre and scale, J. Acoust. Soc. Am. 94(3) 1218-1228: \
                         eq. 1 d(x) = exp(-a x) - exp(-b x) with a = 3.5, b = 5.75; eq. 3 \
                         s = d*/(s1 f_low + s2) with d* = 0.24 and the fit s1 = 0.021, s2 = 19 \
                         as printed; eq. 4 the amplitude factor is the product v1 v2; eq. 5 \
                         roughness sums v1 v2 d(s |f2 - f1|) over every unordered pair";
pub const A: f64 = 3.5;
pub const B: f64 = 5.75;
pub const D_STAR: f64 = 0.24;
pub const S1: f64 = 0.021;
pub const S2: f64 = 19.0;

fn d(x: f64) -> f64 {
    (-A * x).exp() - (-B * x).exp()
}

fn s(f_low: f64) -> f64 {
    D_STAR / (S1 * f_low + S2)
}

pub fn pair(f1: f64, f2: f64, v1: f64, v2: f64) -> f64 {
    v1 * v2 * d(s(f1.min(f2)) * (f2 - f1).abs())
}

/// `pair` over every point of two partials' bounds: `s` falls as `f_low` rises, and `d` rises
/// to its one maximum at ln(b/a)/(b - a) and falls after, so the box's ends bound it.
fn pair_bounds(p: &Partial, q: &Partial) -> (f64, f64) {
    let ((p1, p2), (q1, q2)) = (p.hz_bounds, q.hz_bounds);
    let df = ((p1 - q2).max(q1 - p2).max(0.0), (p2 - q1).max(q2 - p1));
    let x = (s(p2.min(q2)) * df.0, s(p1.min(q1)) * df.1);
    let top = ((B / A).ln() / (B - A)).clamp(x.0, x.1);
    let (va, vb) = (p.amplitude_bounds, q.amplitude_bounds);
    (va.0 * vb.0 * d(x.0).min(d(x.1)), va.1 * vb.1 * d(top))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DissonanceFrame {
    pub t_secs: f64,
    pub roughness: f64,
    /// Over the partials whose amplitude bounds reach the floor, each pair at its bounds.
    pub roughness_bounds: (f64, f64),
    pub partials: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extreme {
    pub value: f64,
    pub t_secs: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dissonance {
    pub framing: Framing,
    pub floor_db: f64,
    pub frames: Vec<DissonanceFrame>,
    pub mean: Option<f64>,
    pub max: Option<Extreme>,
    pub min: Option<Extreme>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crowded {
    pub t_secs: f64,
    pub partials: usize,
    pub pairs: usize,
    pub cap: usize,
}

pub fn measure(
    moments: &[Moment],
    framing: Framing,
    floor_db: f64,
    max_pairs: usize,
) -> Result<Dissonance, Crowded> {
    let floor = 10f64.powf(floor_db / 20.0);
    let mut frames = Vec::with_capacity(moments.len());
    for moment in moments {
        let reach: Vec<&Partial> = moment
            .partials
            .iter()
            .filter(|p| p.amplitude_bounds.1 >= floor)
            .collect();
        let pairs = reach.len() * reach.len().saturating_sub(1) / 2;
        if pairs > max_pairs {
            return Err(Crowded {
                t_secs: moment.t_secs,
                partials: reach.len(),
                pairs,
                cap: max_pairs,
            });
        }
        let (mut roughness, mut low, mut high) = (0.0, 0.0, 0.0);
        for (i, p) in reach.iter().enumerate() {
            for q in &reach[i + 1..] {
                let (lo, hi) = pair_bounds(p, q);
                high += hi;
                if p.amplitude_bounds.0 >= floor && q.amplitude_bounds.0 >= floor {
                    low += lo;
                }
                if p.amplitude >= floor && q.amplitude >= floor {
                    roughness += pair(p.hz, q.hz, p.amplitude, q.amplitude);
                }
            }
        }
        frames.push(DissonanceFrame {
            t_secs: moment.t_secs,
            roughness,
            roughness_bounds: (low, high),
            partials: reach.iter().filter(|p| p.amplitude >= floor).count(),
        });
    }
    let extreme = |better: fn(f64, f64) -> bool| {
        frames
            .iter()
            .fold(None, |held: Option<Extreme>, f| match held {
                Some(e) if !better(f.roughness, e.value) => Some(e),
                _ => Some(Extreme {
                    value: f.roughness,
                    t_secs: f.t_secs,
                }),
            })
    };
    Ok(Dissonance {
        framing,
        floor_db,
        mean: (!frames.is_empty())
            .then(|| frames.iter().map(|f| f.roughness).sum::<f64>() / frames.len() as f64),
        max: extreme(|a, b| a > b),
        min: extreme(|a, b| a < b),
        frames,
    })
}
