// Concern: sliding windows over an envelope on the shared time base, why none fits, stats | Non-concern: what a window reads | IO: (held, rates, hop, window) -> spans

use super::harmony::dissonance::Extreme;

/// `least_secs` is twice the longest period read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unread {
    WindowUnderTwoPeriods {
        window_secs: f64,
        least_secs: f64,
    },
    TargetShorterThanWindow {
        held_secs: f64,
        window_secs: f64,
        least_secs: f64,
    },
}

pub fn fits(
    held: usize,
    sample_rate: f64,
    window_secs: f64,
    longest_period_secs: f64,
) -> Result<(), Unread> {
    let least_secs = 2.0 * longest_period_secs;
    if window_secs < least_secs {
        return Err(Unread::WindowUnderTwoPeriods {
            window_secs,
            least_secs,
        });
    }
    let held_secs = held as f64 / sample_rate;
    if held_secs < window_secs {
        return Err(Unread::TargetShorterThanWindow {
            held_secs,
            window_secs,
            least_secs,
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub t_secs: f64,
    pub from_secs: f64,
    pub to_secs: f64,
    pub from: usize,
    pub to: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spans {
    pub hop_secs: f64,
    pub window_secs: f64,
    pub spans: Vec<Span>,
}

/// Centres step by `hop_secs` from the start as every framed reading steps; each window is
/// clipped to the envelope's `len` samples at `rate`.
pub fn spans(
    held: usize,
    sample_rate: f64,
    start_secs: f64,
    hop_secs: f64,
    rate: f64,
    window_secs: f64,
    len: usize,
) -> Spans {
    let hop = ((hop_secs * sample_rate).round() as usize).max(1);
    let size = (window_secs * rate).round() as usize;
    Spans {
        hop_secs: hop as f64 / sample_rate,
        window_secs: size as f64 / rate,
        spans: (0..held)
            .step_by(hop)
            .map(|c| {
                let middle = (c as f64 * rate / sample_rate).round() as usize;
                let from = middle.saturating_sub(size / 2);
                let to = (middle + size - size / 2).min(len);
                Span {
                    t_secs: start_secs + c as f64 / sample_rate,
                    from_secs: start_secs + from as f64 / rate,
                    to_secs: start_secs + to as f64 / rate,
                    from,
                    to,
                }
            })
            .collect(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub mean: f64,
    pub min: Extreme,
    pub max: Extreme,
}

pub fn stats(values: impl Iterator<Item = (f64, f64)>) -> Option<Stats> {
    let held: Vec<(f64, f64)> = values.collect();
    let first = *held.first()?;
    let pick = |better: fn(f64, f64) -> bool| {
        let (t, value) = held
            .iter()
            .copied()
            .fold(first, |a, b| if better(b.1, a.1) { b } else { a });
        Extreme { value, t_secs: t }
    };
    Some(Stats {
        mean: held.iter().map(|(_, v)| v).sum::<f64>() / held.len() as f64,
        min: pick(|a, b| a < b),
        max: pick(|a, b| a > b),
    })
}
