// Concern: bounds every later sample of one chaigne_askenfelt call site, chunk by chunk | Non-concern: the bound's math (string_tail.rs) | IO: (site states, chunk maxima) -> bounds

use crate::machine::Heard;
use crate::physics::chaigne_askenfelt::{ChaigneAskenfeltParams, ChaigneAskenfeltSite};
use crate::physics::string_tail::{Ringdown, Rung, Settling, Unringing, energy, settling};
use crate::physics::unison_tail::{unison_energy, unison_settling};
use crate::physics::{Solver, Tail};

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

/// What a felted or unison site's energy bound reads; its parameters alone set it.
fn proof(site: &ChaigneAskenfeltSite) -> Result<(Settling, f64), Unringing> {
    let settled = match site.strings.as_slice() {
        [grid] => settling(grid, &site.felt[0])?,
        _ => unison_settling(site)?,
    };
    Ok((settled, site.energy_gain().ok_or(Unringing::Lossless)?))
}

fn pressed(site: &ChaigneAskenfeltSite) -> bool {
    site.landing.is_none_or(|(at, _)| site.steps > at)
}

/// `at(q)` bounds every sample from step `q * step` on. Chunks are heard exactly until every
/// anvil lets go and any felt has pressed; after, a bare string's modes bound the rest, else
/// each chunk's energy does, held once under the level.
#[derive(Clone, Debug)]
pub struct Walk {
    step: usize,
    level: Option<f64>,
    chunks: usize,
    /// The state at the start of chunk `chunks` was read.
    seen: bool,
    heard: Vec<f64>,
    freed: Option<Freed>,
    failed: Option<String>,
}

#[derive(Clone, Debug)]
struct Freed {
    at: usize,
    after: After,
    /// From chunk `at + k` on.
    bounds: Vec<f64>,
    held: Option<usize>,
    /// The loudest chunk from each heard one on.
    later: Vec<f64>,
}

#[derive(Clone, Debug)]
enum After {
    Energy {
        settled: Settling,
        c: f64,
        unison: bool,
        ramped: u64,
    },
    Ring(Rung),
}

impl Walk {
    pub fn new(step: usize) -> Walk {
        Walk {
            step,
            level: None,
            chunks: 0,
            seen: false,
            heard: Vec::new(),
            freed: None,
            failed: None,
        }
    }

    /// No state before a felt lands is freed, the one it stands at included.
    pub fn before_landing(step: usize, heard: &[f64]) -> Walk {
        Walk {
            chunks: heard.len(),
            seen: true,
            heard: heard.to_vec(),
            ..Walk::new(step)
        }
    }

    pub fn chunks(&self) -> usize {
        self.chunks
    }

    pub fn set_level(&mut self, level: f64) {
        self.level = Some(level);
        if let Some(freed) = &mut self.freed
            && let After::Energy { .. } = freed.after
            && freed.held.is_none()
            && let Some(k) = freed.bounds.iter().position(|b| *b < level)
        {
            freed.held = Some(k);
            freed.bounds.truncate(k + 1);
        }
    }

    /// Nothing later is read: every chunk's bound is known.
    pub fn done(&self) -> bool {
        self.failed.is_some()
            || self
                .freed
                .as_ref()
                .is_some_and(|f| matches!(f.after, After::Ring(_)) || f.held.is_some())
    }

    pub fn wants(&self) -> bool {
        if self.seen || self.failed.is_some() {
            return false;
        }
        match &self.freed {
            None => true,
            Some(freed) => matches!(freed.after, After::Energy { .. }) && freed.held.is_none(),
        }
    }

    /// `site` as it stands `count` steps in, `chunks` the loudest sample of each whole chunk
    /// it stepped: each chunk not yet heard is, and the state it stands at is read.
    pub fn hear(&mut self, site: &dyn Solver, count: u64, chunks: &[f64]) -> Result<(), String> {
        let step = self.step as u64;
        if step == 0 || !count.is_multiple_of(step) {
            return Ok(());
        }
        let at = (count / step) as usize;
        while self.chunks < at && !self.wants() {
            self.chunk(chunks[self.chunks]);
        }
        match self.chunks == at && self.wants() {
            true => self.boundary(site),
            false => Ok(()),
        }
    }

    fn boundary(&mut self, site: &dyn Solver) -> Result<(), String> {
        let Some(site) = site.as_any().downcast_ref::<ChaigneAskenfeltSite>() else {
            return Err("another model's site".to_string());
        };
        if !self.wants() {
            return Ok(());
        }
        self.seen = true;
        let freed = match self.freed.take() {
            Some(freed) => freed,
            None if site.let_go() && pressed(site) => match self.free(site) {
                Ok(freed) => freed,
                Err(why) => {
                    self.failed = Some(why.clone());
                    return Err(why);
                }
            },
            None => return Ok(()),
        };
        let mut freed = freed;
        if let After::Energy {
            settled,
            c,
            unison,
            ramped,
        } = &freed.after
        {
            let upper = match unison {
                true => unison_energy(site).1,
                false => energy(&site.strings[0], site.dt, site.springs(0)).1,
            };
            let left = ramped.saturating_sub(site.steps);
            let bound = settled.bound(*c, upper, left);
            freed.bounds.push(bound);
            if self.level.is_some_and(|level| bound < level) {
                freed.held = Some(freed.bounds.len() - 1);
            }
        }
        self.freed = Some(freed);
        Ok(())
    }

    fn free(&self, site: &ChaigneAskenfeltSite) -> Result<Freed, String> {
        let unison = site.strings.len() > 1;
        let after = match unison || site.landing.is_some() {
            true => {
                let (settled, c) = site
                    .proven
                    .get_or_init(|| proof(site))
                    .map_err(|why| unringing(why, unison))?;
                let ramped = site.landing.map_or(0, |(at, ramp)| at + ramp.ceil() as u64);
                After::Energy {
                    settled,
                    c,
                    unison,
                    ramped,
                }
            }
            false => {
                let gain = site.tensions[0] / site.strings[0].dx;
                let ring =
                    Ringdown::of(&site.strings[0], gain).map_err(|why| unringing(why, false))?;
                After::Ring(ring.rung(self.step))
            }
        };
        let mut later = self.heard.clone();
        for k in (0..later.len().saturating_sub(1)).rev() {
            later[k] = later[k].max(later[k + 1]);
        }
        let mut freed = Freed {
            at: self.chunks,
            after,
            bounds: Vec::new(),
            held: None,
            later,
        };
        if let After::Ring(rung) = &mut freed.after {
            freed.bounds.push(rung.next().expect("a ring reads on"));
        }
        Ok(freed)
    }

    fn chunk(&mut self, largest: f64) {
        if self.freed.is_none() {
            self.heard.push(largest);
        }
        self.chunks += 1;
        self.seen = false;
    }

    pub fn at(&mut self, q: usize) -> Option<Result<f64, String>> {
        if let Some(why) = &self.failed {
            return Some(Err(why.clone()));
        }
        let freed = self.freed.as_mut()?;
        let first = freed.bounds[0];
        if q < freed.at {
            return Some(Ok(first.max(freed.later[q])));
        }
        let k = q - freed.at;
        if let Some(h) = freed.held
            && k >= h
        {
            return Some(Ok(freed.bounds[h]));
        }
        if let After::Ring(rung) = &mut freed.after {
            while freed.bounds.len() <= k {
                freed.bounds.push(rung.next().expect("a ring reads on"));
            }
        }
        freed.bounds.get(k).copied().map(Ok)
    }

    /// The chunks a walk from rest steps before `at(q)` is known.
    pub fn cost(&self, q: usize) -> Option<usize> {
        let freed = self.freed.as_ref()?;
        Some(match (&freed.after, freed.held) {
            _ if q <= freed.at => freed.at,
            (After::Ring(_), _) => freed.at,
            (After::Energy { .. }, Some(h)) => q.min(freed.at + h),
            (After::Energy { .. }, None) => q,
        })
    }

    pub fn held(&self) -> Option<usize> {
        let freed = self.freed.as_ref()?;
        freed.held.map(|h| freed.at + h)
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

/// Every chunk unbounded where the site is not freed inside `points`.
pub fn tail_from(
    from: &ChaigneAskenfeltSite,
    step: usize,
    points: usize,
    level: f64,
) -> Result<Tail, String> {
    if step == 0 {
        return Err("a tail bound on a grid with no step".to_string());
    }
    let mut walk = Walk::new(step);
    walk.set_level(level);
    let (mut site, mut heard) = (from.clone(), Heard::every(step as u64));
    let mut at = Vec::with_capacity(points);
    for q in 0..points {
        loop {
            walk.hear(&site, heard.count(), heard.chunks())?;
            if let Some(known) = walk.at(q) {
                at.push(known?);
                break;
            }
            if walk.chunks() + 1 >= points {
                return Ok(Tail {
                    at: vec![f64::INFINITY; points],
                    held: false,
                });
            }
            for _ in 0..step {
                heard.note(site.advance());
            }
        }
    }
    let held = walk.held().is_some_and(|h| h + 1 < points);
    Ok(Tail { at, held })
}
