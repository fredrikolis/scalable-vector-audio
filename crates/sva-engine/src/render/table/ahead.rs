// Concern: steps each stored value's live remainder ahead of its readers, to stand where the stored samples end when they do | Non-concern: loading those samples | IO: (Table, window, live) -> Ahead

use sva_samples::{Extent, Machine, Tape};

use super::segments::Segments;
use super::value::{Held, Kind};
use super::{Pulled, Table};
use crate::cache::Recording;
use crate::error::EngineError;

/// Remainder samples stepped per sample played, once it must start.
const PACE: i64 = 2;

/// Its cost, and each value a live stream started silent.
#[derive(Default)]
pub(crate) struct Ahead {
    pub(crate) pulled: Pulled,
    pub(crate) dropped: Vec<usize>,
}

impl Table {
    /// A live stream steps a remainder at most `PACE + 1` blocks a block and starts silent one
    /// a block reads short; an exact one computes whatever it takes.
    pub(crate) fn ahead(
        &mut self,
        (window, last): (Extent, i64),
        live: bool,
        recording: &mut Recording,
    ) -> Result<Ahead, EngineError> {
        let remainders = self.remainders();
        if remainders.is_empty() {
            return Ok(Ahead::default());
        }
        let needs = self.demand(window);
        let rest = self.demand(Extent::new(window.start, last.max(window.end)));
        let mut asks = Vec::new();
        for (stored, remainder, start) in &remainders {
            let asked = &needs[*stored].hold;
            let covers = self.values[*stored].covers();
            if rest[*stored].hold.minus(&covers).is_empty() {
                continue;
            }
            let Some(due) = due(&covers, asked) else {
                continue;
            };
            let from = self.values[*remainder].end().unwrap_or(*start);
            let mut to = due;
            if live {
                to = to.min(from.saturating_add((PACE + 1) * window.len() as i64));
            }
            if to > from {
                asks.push((*remainder, Extent::new(from, to)));
            }
        }
        let pulled = self.pulled(&asks, recording)?;
        let mut dropped = Vec::new();
        if live {
            let needs = self.demand(window);
            for (stored, remainder, start) in remainders {
                let first = needs[stored].compute.hull().start;
                let from = self.values[remainder].end().unwrap_or(start);
                if !needs[stored].compute.is_empty()
                    && from < first
                    && self.silent(remainder, first)
                {
                    dropped.push(remainder);
                }
            }
            if !dropped.is_empty() {
                for at in 0..self.values.len() {
                    let reads_impure = self.values[at].reads.iter().any(|r| !self.values[*r].pure);
                    self.values[at].pure &= !reads_impure;
                }
            }
        }
        Ok(Ahead { pulled, dropped })
    }

    fn remainders(&self) -> Vec<(usize, usize, i64)> {
        let mut out = Vec::new();
        for (at, value) in self.values.iter().enumerate() {
            let (Kind::Stored { .. }, [remainder]) = (&value.kind, value.reads.as_slice()) else {
                continue;
            };
            if let Kind::Program(program) = &self.values[*remainder].kind
                && let Some(start) = program.start
            {
                out.push((at, *remainder, start));
            }
        }
        out
    }

    /// Silent before `from`, stepping on from there.
    fn silent(&mut self, at: usize, from: i64) -> bool {
        let value = &mut self.values[at];
        let Kind::Program(program) = &mut value.kind else {
            return false;
        };
        let Ok(machine) = Machine::over(&program.spanned, from) else {
            return false;
        };
        program.machine = Some(machine);
        program.marks.clear();
        value.held = Held::Run(Tape::new(value.width, 0, from));
        value.support = value.support.intersect(Extent::from(from));
        value.pure = false;
        true
    }
}

/// Per asked run starting in a stored run: that run's end less `PACE` times what is left.
fn due(covers: &Segments, asked: &Segments) -> Option<i64> {
    asked
        .iter()
        .filter_map(|run| {
            let stored = covers
                .iter()
                .find(|e| e.start <= run.start && run.start < e.end)?;
            Some(stored.end - PACE * (stored.end - run.end))
        })
        .max()
}
