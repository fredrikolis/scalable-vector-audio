// Concern: which old value each value of an edited table carries on, and from where | Non-concern: building either table | IO: (new table, old table, now) -> the new table carried, those started silent

use std::collections::HashMap;

use sva_samples::{Extent, Machine, NodeRenderer, Slot, Tape};

use super::Table;
use super::program::leaves;
use super::value::{Held, Kind, Value};

/// Readers first, each value carries on the old value of its key whole; a stateful one no old
/// value names takes the state of the value its carried reader read through the same map,
/// where its call sites take it. One that takes none steps again from its start, or, `live`,
/// starts silent where the stream stands: those are returned.
pub(crate) fn carried(new: &mut Table, old: Table, now: i64, live: bool) -> Vec<usize> {
    let keys: HashMap<_, usize> = old
        .values
        .iter()
        .enumerate()
        .map(|(at, v)| (v.key, at))
        .collect();
    let old_root = old.root;
    let shapes: Vec<Shape> = old
        .values
        .iter()
        .map(|v| (v.reads.clone(), reads(v)))
        .collect();
    let mut old: Vec<Option<Value>> = old.values.into_iter().map(Some).collect();
    let count = new.values.len();
    let same: Vec<Option<usize>> = new
        .values
        .iter()
        .map(|v| keys.get(&v.key).copied())
        .collect();
    let kept: Vec<usize> = same.iter().flatten().copied().collect();
    let mut paired: Vec<Option<usize>> = vec![None; count];
    let mut local: Vec<Option<i64>> = vec![None; count];
    paired[new.root] = Some(old_root);
    local[new.root] = Some(now);
    let mut dropped = Vec::new();
    for at in (0..count).rev() {
        let candidates = match same[at] {
            Some(was) => vec![was],
            None => predecessors(new, &shapes, &paired, at, &kept),
        };
        paired[at] = paired[at].or(candidates.first().copied());
        let here = local[at];
        for (slot, map) in reads(&new.values[at]) {
            let read = new.values[at].reads[slot];
            let there = here.map(|n| map.at(n));
            local[read] = local[read].max(there);
        }
        if let Some(was) = same[at] {
            let old = old[was].take().expect("an old value carries on once");
            take(&mut new.values[at], old);
            continue;
        }
        if !stateful(&new.values[at]) {
            continue;
        }
        let at_now = local[at].unwrap_or(now);
        let taken = candidates.iter().find_map(|was| {
            let held = old[*was].as_ref()?;
            continues(&new.values[at], held, at_now).then_some(*was)
        });
        match taken {
            Some(was) => {
                let old = old[was].take().expect("a predecessor carries on once");
                carry(&mut new.values[at], old);
                paired[at] = Some(was);
            }
            None if live && start_silent(&mut new.values[at], at_now) => dropped.push(at),
            None => {}
        }
    }
    for at in 0..count {
        let reads_carried = new.values[at].reads.iter().any(|r| !new.values[*r].pure);
        new.values[at].pure &= !reads_carried;
    }
    dropped
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

/// An old value's reads, and each read's slot and map.
type Shape = (Vec<usize>, Vec<(usize, sva_samples::Map)>);

/// The old values each carried reader of `at` read through the same map, same slot first.
fn predecessors(
    new: &Table,
    old: &[Shape],
    paired: &[Option<usize>],
    at: usize,
    kept: &[usize],
) -> Vec<usize> {
    let mut ranked = Vec::new();
    for (reader, was) in paired.iter().enumerate().skip(at + 1) {
        let Some((before, theirs)) = was.map(|w| &old[w]) else {
            continue;
        };
        for (slot, map) in reads(&new.values[reader]) {
            if new.values[reader].reads[slot] != at {
                continue;
            }
            for (their, their_map) in theirs {
                let read = before[*their];
                if *their_map == map && !kept.contains(&read) {
                    ranked.push((*their != slot, read));
                }
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
    let (Some(machine), Some(end), Held::Run(tape)) = (&was.machine, old.end(), &old.held) else {
        return false;
    };
    let fresh = Machine::over(&program.spanned, end);
    old.width == value.width
        && end <= now
        && tape.base() <= end.saturating_sub(program.own).max(tape.origin())
        && fresh.is_ok_and(|opened| opened.accepts(&machine.state()))
}

fn take(value: &mut Value, old: Value) {
    value.pure = old.pure;
    value.held = old.held;
    value.evaluated = old.evaluated;
    if let (Kind::Program(program), Kind::Program(was)) = (&mut value.kind, old.kind) {
        program.machine = was.machine;
    }
}

/// The old value's samples and state, stepped on under the new program.
fn carry(value: &mut Value, old: Value) {
    let end = old.end().expect("a stateful value stands somewhere");
    let Kind::Program(was) = old.kind else {
        unreachable!("a stateful predecessor is a program");
    };
    let state = was.machine.as_ref().expect("a machine that ran").state();
    let Kind::Program(program) = &mut value.kind else {
        unreachable!("a stateful value is a program");
    };
    let mut machine = Machine::over(&program.spanned, end).expect("its spans opened once already");
    machine.carry(&state);
    program.machine = Some(machine);
    value.held = old.held;
    value.pure = false;
}

/// Nothing of its past computed: silent before `now`, stepping from there.
fn start_silent(value: &mut Value, now: i64) -> bool {
    let end = value.end();
    let Kind::Program(program) = &mut value.kind else {
        return false;
    };
    let start = program.start.expect("a stateful program");
    if now <= start || end.is_some_and(|end| end > start) {
        return false;
    }
    let Ok(machine) = Machine::over(&program.spanned, now) else {
        return false;
    };
    program.machine = Some(machine);
    value.held = Held::Run(Tape::new(value.width, 0, now));
    value.support = value.support.intersect(Extent::from(now));
    value.pure = false;
    true
}
