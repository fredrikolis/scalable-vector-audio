// Concern: bounds every later sample of one chaigne_askenfelt call site | Non-concern: the bound's math (string_tail.rs), stepping the site | IO: (params, sr, step, points, level) -> a bound per instant

use crate::physics::Tail;
use crate::physics::chaigne_askenfelt::{ChaigneAskenfeltParams, ChaigneAskenfeltSite};
use crate::physics::string_tail::{Ringdown, Unringing, energy, energy_gain, settling};
use crate::physics::unison_tail::{unison_energy, unison_gain, unison_settling};

fn unringing(why: Unringing, unison: bool) -> String {
    let what = match unison {
        true => "unison on its bridge",
        false => "string",
    };
    match why {
        Unringing::Lossless => format!("a chaigne_askenfelt {what} with a mode that loses nothing"),
        Unringing::Critical => {
            format!("a chaigne_askenfelt {what} with a mode near critical damping")
        }
        Unringing::Rounding => {
            format!("a chaigne_askenfelt {what} whose rounding outpaces its decay")
        }
    }
}

/// Stepped until every anvil lets go and any felt has pressed, heard exactly until then. An
/// unfelted string's modes bound every sample after; a felted one's energy bounds each grid
/// instant's future, stepped until one falls under `level` and held there.
pub fn tail(
    params: &ChaigneAskenfeltParams,
    sr: f64,
    step: usize,
    points: usize,
    level: f64,
) -> Result<Tail, String> {
    let mut site = ChaigneAskenfeltSite::new(params, sr).map_err(|e| e.to_string())?;
    if step == 0 {
        return Err("a tail bound on a grid with no step".to_string());
    }
    let pressed = |site: &ChaigneAskenfeltSite| site.landing.is_none_or(|(at, _)| site.steps > at);
    let mut heard = Vec::new();
    while !(site.let_go() && pressed(&site) && heard.len() % step == 0)
        && heard.len() < points * step
    {
        heard.push(site.advance().abs());
    }
    if heard.len() >= points * step {
        return Ok(Tail {
            at: vec![f64::INFINITY; points],
            held: false,
        });
    }
    let free = heard.len() / step;
    let mut at = vec![0.0f64; points];
    let mut held = false;
    let gain = site.tensions[0] / site.strings[0].dx;
    let unison = site.strings.len() > 1;
    match site.landing {
        None if !unison => {
            let ring = Ringdown::of(&site.strings[0], gain).map_err(|why| unringing(why, false))?;
            at[free..].copy_from_slice(&ring.along(0, step, points - free));
        }
        landing => {
            let why = |why| unringing(why, unison);
            let (settled, c) = match unison {
                true => (
                    unison_settling(&site).map_err(why)?,
                    unison_gain(&site).ok_or_else(|| why(Unringing::Lossless))?,
                ),
                false => (
                    settling(&site.strings[0], &site.felt[0]).map_err(why)?,
                    energy_gain(&site.strings[0], gain, site.dt),
                ),
            };
            let ramped = landing.map_or(0, |(at, ramp)| at + ramp.ceil() as u64);
            for j in free..points {
                let upper = match unison {
                    true => unison_energy(&site).1,
                    false => energy(&site.strings[0], site.dt, site.springs(0)).1,
                };
                let left = ramped.saturating_sub(site.steps);
                let bound = settled.bound(c, upper, left);
                if bound < level {
                    held = j + 1 < points;
                    at[j..].fill(bound);
                    break;
                }
                at[j] = bound;
                for _ in 0..step {
                    site.advance();
                }
            }
        }
    }
    let mut running = at[free];
    for k in (0..heard.len()).rev() {
        running = running.max(heard[k]);
        if k % step == 0 {
            at[k / step] = running;
        }
    }
    Ok(Tail { at, held })
}
