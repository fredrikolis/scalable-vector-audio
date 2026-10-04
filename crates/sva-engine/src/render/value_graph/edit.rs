// Concern: which value going each value a change made carries the state of | Non-concern: building or letting go of either | IO: (values made, values going, now) -> state carried, started silent

use std::collections::{HashMap, HashSet};

use sva_samples::{Machine, NodeRenderer, Slot};

use super::program::leaves;
use super::value::{Held, Kind, Value};
use super::values::Values;

/// Each value a change started silent, and how many took an old state.
pub(crate) struct Carried {
    pub(crate) silent: Vec<usize>,
    pub(crate) taken: usize,
}

/// Readers first, a stateful value the change made takes the state of the value its carried
/// reader read through the same map, among those the change lets go. One that takes none steps
/// again from its start, or, `live`, starts silent at the first sample any reader reads of it
/// from now on. A value of the same identity is the same value, carried on in its slot.
pub(crate) fn carried(
    values: &mut Values,
    (made, going): (&[usize], &[usize]),
    (old_root, root): (Option<usize>, usize),
    (now, live): (i64, bool),
) -> Carried {
    let going: HashSet<usize> = going.iter().copied().collect();
    let place: HashMap<usize, usize> = made.iter().enumerate().map(|(k, at)| (*at, k)).collect();
    let mapped: Vec<Vec<(usize, sva_samples::Map)>> =
        made.iter().map(|at| reads(&values[*at])).collect();
    let mut readers: Vec<Vec<(usize, usize, sva_samples::Map)>> = vec![Vec::new(); made.len()];
    for (reader, each) in mapped.iter().enumerate() {
        for (slot, map) in each {
            if let Some(read) = place.get(&values[made[reader]].reads[*slot]) {
                readers[*read].push((reader, *slot, *map));
            }
        }
    }
    let mut paired: Vec<Option<usize>> = vec![None; made.len()];
    let (mut latest, mut first): (Vec<Option<i64>>, Vec<Option<i64>>) =
        (vec![None; made.len()], vec![None; made.len()]);
    if let Some(at) = place.get(&root) {
        paired[*at] = old_root;
        (latest[*at], first[*at]) = (Some(now), Some(now));
    }
    let (mut taken, mut out) = (
        HashSet::new(),
        Carried {
            silent: Vec::new(),
            taken: 0,
        },
    );
    for k in (0..made.len()).rev() {
        let at = made[k];
        let mut candidates = predecessors(&readers[k], values, &paired, &going);
        if let Some(was) = paired[k].filter(|was| going.contains(was) && !candidates.contains(was))
        {
            candidates.insert(0, was);
        }
        paired[k] = paired[k].or(candidates.first().copied());
        let went_on = match stateful(&values[at]) {
            false => false,
            true => {
                let at_now = latest[k].unwrap_or(now);
                let was = candidates.iter().copied().find(|was| {
                    !taken.contains(was) && continues(&values[at], &values[*was], at_now)
                });
                if let Some(was) = was {
                    taken.insert(was);
                    let mut lifted = values.lift(at);
                    carry(&mut lifted, &mut values[was]);
                    values.put(at, lifted);
                    paired[k] = Some(was);
                    out.taken += 1;
                } else if live && start_silent(&mut values[at], first[k].unwrap_or(now)) {
                    out.silent.push(at);
                }
                was.is_some()
            }
        };
        let here = latest[k];
        let from = if went_on { here } else { first[k] };
        let mut reach = |read: usize, latest_at: Option<i64>, first_at: Option<i64>| {
            if let Some(r) = place.get(&read) {
                latest[*r] = latest[*r].max(latest_at);
                first[*r] = earliest(first[*r], first_at);
            }
        };
        for (slot, map) in mapped[k].iter().copied() {
            let read = values[at].reads[slot];
            reach(read, here.map(|n| map.at(n)), from.map(|n| map.at(n)));
        }
        for (slot, reached) in indexed(&values[at]) {
            let read = values[at].reads[slot];
            let least = reached.map_or(i64::MIN, |(least, _)| least);
            reach(read, None, from.map(|n| n.saturating_add(least)));
        }
        if let Kind::Resident { .. } = values[at].kind {
            for read in values[at].reads.clone() {
                reach(read, here, from);
            }
        }
    }
    for at in made {
        let reads_carried = values[*at].reads.iter().any(|r| !values[*r].pure);
        values[*at].pure &= !reads_carried;
    }
    out
}

fn earliest(held: Option<i64>, there: Option<i64>) -> Option<i64> {
    match (held, there) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Each read of `value`'s program, by slot and map, where it reads through one.
fn reads(value: &Value) -> Vec<(usize, sva_samples::Map)> {
    let mut out = Vec::new();
    if let Kind::Program(program) = &value.kind {
        leaves(&program.renderer, &mut |leaf| {
            if let NodeRenderer::Read {
                slot: Slot::Read(at),
                map,
            } = leaf
            {
                out.push((at.0 as usize, *map));
            }
        });
    }
    out
}

/// Each index read of `value`'s program, by slot, and its reach where bounded.
fn indexed(value: &Value) -> Vec<(usize, Option<(i64, i64)>)> {
    let mut out = Vec::new();
    if let Kind::Program(program) = &value.kind {
        leaves(&program.renderer, &mut |leaf| {
            if let NodeRenderer::Indexed {
                slot: Slot::Read(at),
                reach,
                ..
            } = leaf
            {
                out.push((at.0 as usize, *reach));
            }
        });
    }
    out
}

/// The values going that `readers`' pairs read through the same map, same slot first.
fn predecessors(
    readers: &[(usize, usize, sva_samples::Map)],
    values: &Values,
    paired: &[Option<usize>],
    going: &HashSet<usize>,
) -> Vec<usize> {
    let mut ranked = Vec::new();
    for (reader, slot, map) in readers {
        let Some(was) = paired[*reader] else {
            continue;
        };
        let before = &values[was];
        for (their, their_map) in reads(before) {
            let read = before.reads[their];
            if their_map == *map && going.contains(&read) {
                ranked.push((their != *slot, read));
            }
        }
    }
    ranked.sort_by_key(|(elsewhere, _)| *elsewhere);
    let mut out = Vec::new();
    for (_, read) in ranked {
        if !out.contains(&read) {
            out.push(read);
        }
    }
    out
}

fn stateful(value: &Value) -> bool {
    matches!(&value.kind, Kind::Program(program) if program.stateful())
}

/// An alike value, stood no later than `now`, holding as much of its past as the new value
/// reads back, whose call sites take the new value's.
fn continues(value: &Value, old: &Value, now: i64) -> bool {
    let (Kind::Program(program), Kind::Program(was)) = (&value.kind, &old.kind) else {
        return false;
    };
    let (Some(machine), Some(end), Held::Run { samples, origin }) =
        (&was.machine, old.end(), &old.held)
    else {
        return false;
    };
    let fresh = Machine::over(&program.spanned, end);
    old.width == value.width
        && end <= now
        && samples.start <= end.saturating_sub(program.own).max(*origin)
        && fresh.is_ok_and(|opened| opened.accepts(&machine.state()))
}

/// The old value's samples and state, stepped on under the new program; the old keeps none.
fn carry(value: &mut Value, old: &mut Value) {
    let end = old.end().expect("a stateful value stands somewhere");
    let Kind::Program(was) = &mut old.kind else {
        unreachable!("a stateful predecessor is a program");
    };
    let state = was.machine.take().expect("a machine that ran").state();
    let Kind::Program(program) = &mut value.kind else {
        unreachable!("a stateful value is a program");
    };
    let mut machine = Machine::over(&program.spanned, end).expect("its spans opened once already");
    machine.carry(&state);
    program.machine = Some(machine);
    value.held = std::mem::replace(&mut old.held, Held::Segments(Vec::new()));
    value.pure = false;
}

/// A fresh stateful value started silent at `now`.
fn start_silent(value: &mut Value, now: i64) -> bool {
    let end = value.end();
    let Kind::Program(program) = &value.kind else {
        return false;
    };
    let start = program.start.expect("a stateful program");
    if now <= start || end.is_some_and(|end| end > start) {
        return false;
    }
    value.silent_from(now).is_ok()
}
