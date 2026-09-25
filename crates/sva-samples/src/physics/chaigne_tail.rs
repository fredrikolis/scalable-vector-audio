// Concern: bounds every later sample of one chaigne_askenfelt call site, from rest or a held state | Non-concern: the bound's math (string_tail.rs) | IO: (params or site, step, points, level) -> bounds

use crate::physics::Tail;
use crate::physics::chaigne_askenfelt::{ChaigneAskenfeltParams, ChaigneAskenfeltSite};
use crate::physics::string_tail::{Ringdown, Settling, Unringing, energy, settling};
use crate::physics::unison_tail::{unison_energy, unison_settling};

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

/// A site at rest, bounded by [`tail_from`].
pub fn tail(
    params: &ChaigneAskenfeltParams,
    sr: f64,
    step: usize,
    points: usize,
    level: f64,
) -> Result<Tail, String> {
    let site = ChaigneAskenfeltSite::new(params, sr).map_err(|e| e.to_string())?;
    tail_from(&site, step, points, level)
}

/// What a felted or unison site's energy bound reads; its parameters alone set it.
fn proof(site: &ChaigneAskenfeltSite) -> Result<(Settling, f64), Unringing> {
    let settled = match site.strings.as_slice() {
        [grid] => settling(grid, &site.felt[0])?,
        _ => unison_settling(site)?,
    };
    Ok((settled, site.energy_gain().ok_or(Unringing::Lossless)?))
}

/// From `from`'s state, a copy stepped until every anvil lets go and any felt has pressed, heard
/// exactly until then; after, an unfelted string's modes bound every sample, and otherwise the
/// energy bounds each instant's future, stepped until one falls under `level` and held there.
pub fn tail_from(
    from: &ChaigneAskenfeltSite,
    step: usize,
    points: usize,
    level: f64,
) -> Result<Tail, String> {
    if step == 0 {
        return Err("a tail bound on a grid with no step".to_string());
    }
    let unison = from.strings.len() > 1;
    let mut site = from.clone();
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
    let proven = match unison || from.landing.is_some() {
        true => {
            let found = *from.proven.get_or_init(|| proof(from));
            Some(found.map_err(|why| unringing(why, unison))?)
        }
        false => None,
    };
    let free = heard.len() / step;
    let mut at = vec![0.0f64; points];
    let mut held = false;
    match proven {
        None => {
            let gain = site.tensions[0] / site.strings[0].dx;
            let ring = Ringdown::of(&site.strings[0], gain).map_err(|why| unringing(why, false))?;
            at[free..].copy_from_slice(&ring.along(0, step, points - free));
        }
        Some((settled, c)) => {
            let ramped = site.landing.map_or(0, |(at, ramp)| at + ramp.ceil() as u64);
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
                if j + 1 < points {
                    for _ in 0..step {
                        site.advance();
                    }
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
