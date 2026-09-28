// Concern: which old node each node of an edited expression carries on, and from where | Non-concern: building either set | IO: (new, old nodes, pairs, now) -> the new nodes, carried in place

use std::collections::BTreeSet;

use sva_samples::{Extent, Tape};

use super::node::{Driven, Kind};

enum Carry {
    Taken,
    Unlike,
    /// Alike, but the old node no longer holds what is asked of it.
    Unheld,
}

/// Readers first, each node carries on the old node of its identity, else a run in the store,
/// else the node its carried reader read at the same shift where the structure takes its
/// state, else starts at `now`. One whose predecessor lacks what is asked steps again, or,
/// `live`, starts silent at `now` where that means computing its past: those are returned.
pub(in crate::render) fn carried(
    nodes: &mut [Driven],
    old: &mut [Driven],
    same: &[Option<usize>],
    roots: (Option<usize>, Option<usize>),
    (now, live): (i64, bool),
) -> Vec<usize> {
    let mut dropped = Vec::new();
    let mut readers: Vec<Vec<(usize, i64)>> = vec![Vec::new(); nodes.len()];
    for (r, node) in nodes.iter().enumerate() {
        for &(read, shift) in &node.reads {
            readers[read].push((r, shift));
        }
    }
    let kept: BTreeSet<usize> = same.iter().flatten().copied().collect();
    let mut paired: Vec<Option<usize>> = vec![None; nodes.len()];
    let mut next: Vec<i64> = vec![i64::MAX; nodes.len()];
    for at in (0..nodes.len()).rev() {
        if matches!(nodes[at].kind, Kind::Ended) {
            continue;
        }
        let local = now - nodes[at].lag;
        let need = readers[at]
            .iter()
            .map(|&(reader, shift)| next[reader].saturating_add(shift))
            .min()
            .unwrap_or(local);
        if need >= nodes[at].extent.end && Some(at) != roots.0 {
            nodes[at].kind = Kind::Ended;
            continue;
        }
        let carry = match same[at] {
            Some(was) => nodes[at].continues(&mut old[was], local, need, true),
            None if nodes[at].run.as_ref().is_some_and(|run| run.loaded) => Carry::Taken,
            None => {
                let mut candidates: Vec<usize> = Vec::new();
                if Some(at) == roots.0 {
                    candidates.extend(roots.1);
                }
                for &(reader, shift) in &readers[at] {
                    if let Some(was) = paired[reader] {
                        let read = old[was].reads.iter().filter(|(_, s)| *s == shift);
                        candidates.extend(read.map(|(node, _)| *node));
                    }
                }
                let mut carry = Carry::Unlike;
                for was in candidates.into_iter().filter(|was| !kept.contains(was)) {
                    match nodes[at].continues(&mut old[was], local, need, false) {
                        Carry::Taken => {
                            paired[at] = Some(was);
                            carry = Carry::Taken;
                            break;
                        }
                        Carry::Unheld => carry = Carry::Unheld,
                        Carry::Unlike => {}
                    }
                }
                carry
            }
        };
        match carry {
            Carry::Taken => paired[at] = paired[at].or(same[at]),
            Carry::Unheld if live && !nodes[at].stateless() && local > nodes[at].extent.start => {
                nodes[at].starts_at(local);
                dropped.push(at);
            }
            Carry::Unheld => nodes[at].steps_again(need),
            Carry::Unlike => nodes[at].starts_at(local),
        }
        next[at] = nodes[at].tape.end();
    }
    dropped
}

impl Driven {
    /// The same node takes all `old` holds; an alike one its samples up to `at` and its state
    /// there. Either needs every sample from `need` on, and what it reads of itself.
    fn continues(&mut self, old: &mut Driven, at: i64, need: i64, same: bool) -> Carry {
        let from = match same {
            true => old.tape.end(),
            false => at.min(old.tape.end()),
        };
        let reach = need.min(from - self.own as i64).max(self.extent.start);
        let alike = match (&self.kind, &old.kind) {
            (_, Kind::Ended) => return Carry::Unheld,
            (Kind::Machine { .. }, Kind::Machine { .. }) => true,
            (Kind::Rows(_) | Kind::Point(_), Kind::Rows(_) | Kind::Point(_)) => true,
            (Kind::Whole, Kind::Whole) => same,
            _ => false,
        };
        if old.width != self.width || !alike {
            return Carry::Unlike;
        }
        if old.tape.base() > reach || from < old.tape.base() {
            return Carry::Unheld;
        }
        let state = (!same).then(|| old.state_at(from)).flatten();
        if let (Kind::Machine { machine, .. }, Kind::Machine { machine: was, .. }) =
            (&mut self.kind, &mut old.kind)
        {
            match state {
                _ if same => std::mem::swap(machine, was),
                Some(state) if machine.accepts(&state) && machine.carry(&state).is_ok() => {}
                Some(_) => return Carry::Unlike,
                None => return Carry::Unheld,
            }
        }
        let mut tape = std::mem::replace(&mut old.tape, Tape::new(old.width, 0, old.extent.end));
        tape.cut(from);
        let origin = tape.base() == self.extent.start;
        self.tape = tape;
        let was = old.run.take();
        if let Some(run) = &mut self.run {
            let agrees = was
                .as_ref()
                .is_some_and(|was| was.records && (same || run.shares(was, from)));
            run.records &= origin && agrees;
            if let (true, Some(was)) = (same, was) {
                run.continues(was);
            }
        }
        old.kind = Kind::Ended;
        Carry::Taken
    }

    fn stateless(&self) -> bool {
        match &self.kind {
            Kind::Machine { machine, .. } => !machine.stateful() && self.own == 0,
            _ => true,
        }
    }

    /// A node holding no state of its own starts where its readers first read.
    fn steps_again(&mut self, need: i64) {
        let start = match self.stateless() {
            true => need.max(self.extent.start),
            false => self.extent.start,
        };
        self.tape = Tape::new(self.width, self.tape.capacity(), start);
        if let Some(run) = &mut self.run {
            run.records &= start == self.extent.start;
        }
    }

    fn starts_at(&mut self, at: i64) {
        if at <= self.extent.start || self.tape.end() > self.extent.start {
            return;
        }
        self.tape = Tape::new(self.width, self.tape.capacity(), at);
        self.support = self.support.intersect(Extent::from(at));
        if let Some(run) = &mut self.run {
            run.records = false;
        }
    }
}
