// Concern: bounds every later sample of one chaigne_askenfelt call site | Non-concern: the bound's math (string_tail.rs), stepping the site | IO: (params, sr, step, points) -> a bound per instant

use crate::physics::chaigne_askenfelt::{ChaigneAskenfeltParams, ChaigneAskenfeltSite};
use crate::physics::string_tail::{Ringdown, Unringing, energy, energy_gain, settling};

const UNISON: &str = "a chaigne_askenfelt unison on its bridge, whose coupling holds no exact \
    discrete energy: the bridge takes the strings' tension but not the bending and \
    frequency-dependent loss their ghost points exert on it";

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
/// instant's future.
pub fn tail(
    params: &ChaigneAskenfeltParams,
    sr: f64,
    step: usize,
    points: usize,
) -> Result<Vec<f64>, String> {
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
        return Ok(vec![f64::INFINITY; points]);
    }
    let free = heard.len() / step;
    let mut at = vec![0.0f64; points];
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
            for slot in at.iter_mut().skip(free) {
                let (_, upper) = energy(&site.strings[0], site.dt, site.springs(0));
                let left = ramped.saturating_sub(site.steps) as f64;
                *slot = c * upper.sqrt() * settled.per_step.powf(left) * settled.after;
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
    Ok(at)
}
