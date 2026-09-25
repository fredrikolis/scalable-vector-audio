// Concern: bounds every later sample of one chaigne_askenfelt call site | Non-concern: the bound's math (string_tail.rs), stepping the site | IO: (params, sr, step, points, level) -> a bound per instant

use crate::physics::Tail;
use crate::physics::chaigne_askenfelt::{ChaigneAskenfeltParams, ChaigneAskenfeltSite};
use crate::physics::string_tail::{Ringdown, Unringing, energy, energy_gain, settling};

const UNISON: &str = "a chaigne_askenfelt unison on its bridge, whose energy is exact but whose \
    settling under rounding is not derived yet";

fn unringing(why: Unringing) -> String {
    match why {
        Unringing::Lossless => "a chaigne_askenfelt string with a mode that loses nothing",
        Unringing::Critical => "a chaigne_askenfelt string with a mode near critical damping",
        Unringing::Rounding => "a chaigne_askenfelt string whose rounding outpaces its decay",
    }
    .to_string()
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
    if site.strings.len() > 1 {
        return Err(UNISON.to_string());
    }
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
    match site.landing {
        None => {
            let ring = Ringdown::of(&site.strings[0], gain).map_err(unringing)?;
            at[free..].copy_from_slice(&ring.along(0, step, points - free));
        }
        Some((landing, ramp)) => {
            let settled = settling(&site.strings[0], &site.felt[0]).map_err(unringing)?;
            let c = energy_gain(&site.strings[0], gain, site.dt);
            let ramped = landing + ramp.ceil() as u64;
            for j in free..points {
                let (_, upper) = energy(&site.strings[0], site.dt, site.springs(0));
                let left = ramped.saturating_sub(site.steps) as f64;
                let bound = c * upper.sqrt() * settled.per_step.powf(left) * settled.after;
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
