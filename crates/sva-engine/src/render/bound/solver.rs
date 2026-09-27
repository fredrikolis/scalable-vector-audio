// Concern: bounds one solver's samples from each grid instant on, through its walk | Non-concern: the walk's math (sva-samples) | IO: (chunk) -> a bound, or the chunk it waits on

use sva_samples::{Machine, Params, Tape, Walk};

use super::envelope::STEP;

pub(crate) struct Solver {
    pub(crate) walk: Walk,
    /// Its own machine, played ahead of what reads it.
    pub(crate) ahead: Option<Played>,
    pub(crate) params: Params,
    charged: usize,
    pub(crate) lost: bool,
}

impl Solver {
    pub(super) fn of(params: &Params) -> Result<Solver, String> {
        match params {
            Params::ChaigneAskenfelt(_) => Ok(Solver {
                walk: Walk::new(STEP),
                ahead: None,
                params: params.clone(),
                charged: 0,
                lost: false,
            }),
            other => Err(format!("the {} solver", other.name())),
        }
    }
}

pub(super) enum Found {
    Value {
        v: f64,
        before: Option<f64>,
        ops: u128,
    },
    Wait(usize),
    Unbounded(String),
}

/// Each chunk a walk from rest steps is charged once, whoever stepped it.
pub(super) fn point(tail: &mut Solver, q: usize, first: bool) -> Found {
    let wanted = match first {
        true => vec![0, q],
        false => vec![q],
    };
    if !tail.lost
        && let Some(c) = wanted.iter().find(|c| tail.walk.at(**c).is_none())
    {
        return Found::Wait(*c);
    }
    if tail.lost {
        return Found::Value {
            v: f64::INFINITY,
            before: first.then_some(f64::INFINITY),
            ops: 0,
        };
    }
    let mut read = |c: usize| tail.walk.at(c).expect("a chunk reached");
    let v = match read(q) {
        Ok(v) => v,
        Err(why) => return Found::Unbounded(why),
    };
    let before = match first {
        true => match read(0) {
            Ok(v) => Some(v),
            Err(why) => return Found::Unbounded(why),
        },
        false => None,
    };
    let cost = wanted
        .iter()
        .filter_map(|c| tail.walk.cost(*c))
        .max()
        .unwrap_or(0);
    let ops = (cost.saturating_sub(tail.charged) * STEP) as u128;
    tail.charged = tail.charged.max(cost);
    Found::Value { v, before, ops }
}

pub(crate) fn hear(machine: &Machine, walk: &mut Walk) {
    if let (Some(site), Some(heard)) = (machine.solver(0), machine.heard(0)) {
        let _ = walk.hear(site, heard.count(), heard.chunks());
    }
}

pub(crate) type Fresh<'f> = &'f dyn Fn() -> Option<Played>;

/// A solver's own machine and what it wrote.
#[derive(Clone)]
pub(crate) struct Played {
    pub(crate) machine: Machine,
    pub(crate) tape: Tape,
}

impl Played {
    fn count(&self) -> u64 {
        self.machine.heard(0).map_or(0, |h| h.count())
    }

    /// `false` past `limit` chunks, or where the walk missed a state the machine passed.
    fn reach(&mut self, walk: &mut Walk, chunk: usize, limit: usize) -> bool {
        let step = STEP as u64;
        loop {
            hear(&self.machine, walk);
            if walk.at(chunk).is_some() {
                return true;
            }
            let count = self.count();
            let behind = count.is_multiple_of(step) && (walk.chunks() as u64) < count / step;
            if behind || walk.chunks() >= limit {
                return false;
            }
            let to = self.tape.end() + (step - count % step) as i64;
            if self.machine.run_to(to, &[], &mut self.tape).is_err() {
                return false;
            }
        }
    }
}

/// The machine played ahead, or a `fresh` one where the walk stands, played on until the walk
/// knows `chunk`; lost past `limit` chunks or with none.
pub(crate) fn play(tail: &mut Solver, chunk: usize, fresh: Fresh, limit: usize) {
    let walked = tail.walk.chunks() as u64;
    let aligned = |p: &Played| p.count() / STEP as u64 == walked;
    if !tail.ahead.as_ref().is_some_and(aligned) {
        tail.ahead = fresh().filter(aligned);
    }
    let walk = &mut tail.walk;
    if !tail
        .ahead
        .as_mut()
        .is_some_and(|p| p.reach(walk, chunk, limit))
    {
        tail.lost = true;
    }
}
