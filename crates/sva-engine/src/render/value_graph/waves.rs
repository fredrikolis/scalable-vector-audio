// Concern: computes a pull's values wave by wave, a value or a stateless one's grid block per task | Non-concern: what a value computes, keeping it | IO: (values, needs, limit) -> values

use std::num::NonZeroUsize;

use sva_samples::{Buffer, Extent, block_end};

use super::ValueGraph;
use super::demand::Need;
use super::eval::{self, Marks};
use super::store;
use super::value::Value;
use super::values::Values;
use crate::cache::{Memory, Recording};
use crate::error::EngineError;

/// `asked` in waves, each after every wave holding a value it reads, `asked`'s order kept within.
pub(super) fn waves(values: &Values, asked: &[usize]) -> Vec<Vec<usize>> {
    let mut depth = vec![0usize; values.span()];
    for at in values.ordered() {
        depth[at] = values[at]
            .reads
            .iter()
            .map(|r| depth[*r] + 1)
            .max()
            .unwrap_or(0);
    }
    let mut waves: Vec<Vec<usize>> = Vec::new();
    for at in asked.iter().copied() {
        let deep = depth[at];
        if waves.len() <= deep {
            waves.resize_with(deep + 1, Vec::new);
        }
        waves[deep].push(at);
    }
    waves.retain(|wave| !wave.is_empty());
    waves
}

enum Task<'v> {
    Whole(usize, &'v mut Value, &'v Need, &'v Marks),
    Block(usize, &'v Value, Extent),
}

enum Done {
    Whole(usize, Result<(), EngineError>),
    Block(usize, Extent, Result<Buffer, EngineError>),
}

impl ValueGraph {
    /// Computes each of `wave`, none reading another, on at most `limit` threads: the first
    /// refusal in the wave's order, each value holding what it computed before its own.
    pub(super) fn computed(
        &mut self,
        wave: &[usize],
        needs: &[Need],
        (memory, seen): (&Memory, &mut Recording),
        limit: NonZeroUsize,
    ) -> Result<(), EngineError> {
        let marks: Vec<Marks> = wave
            .iter()
            .map(|at| store::marks(self.values.place(*at), memory))
            .collect();
        let mut lifted: Vec<Value> = wave.iter().map(|at| self.values.lift(*at)).collect();
        let (done, profile) = (&self.values, &self.profile);
        let mut tasks = Vec::new();
        let mut blocks = Vec::new();
        for (k, value) in lifted.iter_mut().enumerate() {
            let need = &needs[wave[k]];
            if !eval::apart(value) {
                tasks.push(Task::Whole(k, value, need, &marks[k]));
                continue;
            }
            let value: &Value = value;
            for segment in need.compute.iter() {
                let mut from = segment.start;
                while from < segment.end {
                    let to = block_end(from).min(segment.end);
                    blocks.push(Task::Block(k, value, Extent::new(from, to)));
                    from = to;
                }
            }
        }
        tasks.append(&mut blocks);
        let answers = crate::threads::each(limit, tasks, |task| match task {
            Task::Whole(k, value, need, marks) => {
                Done::Whole(k, eval::compute(value, need, (done, marks), profile))
            }
            Task::Block(k, value, segment) => {
                Done::Block(k, segment, eval::segment(value, segment, done))
            }
        });
        let mut refused: Option<(usize, EngineError)> = None;
        let mut failed = vec![false; wave.len()];
        for answer in answers {
            let failure = match answer {
                Done::Whole(k, done) => done.err().map(|e| (k, e)),
                Done::Block(k, _, _) if failed[k] => None,
                Done::Block(k, segment, Ok(samples)) => {
                    eval::held(&mut lifted[k], segment, samples);
                    None
                }
                Done::Block(k, _, Err(e)) => {
                    failed[k] = true;
                    Some((k, e))
                }
            };
            if let Some((k, e)) = failure
                && refused.as_ref().is_none_or(|(first, _)| k < *first)
            {
                refused = Some((k, e));
            }
        }
        for (at, value) in wave.iter().zip(lifted) {
            self.values.put(*at, value);
        }
        if let Some((_, refused)) = refused {
            return Err(refused);
        }
        for at in wave.iter().copied() {
            let (value, place) = self.values.placed(at);
            for (read, count) in store::reached(value, place, &needs[at].compute) {
                let (read, place) = self.values.placed(read);
                store::reread(read, place, count, seen);
            }
        }
        Ok(())
    }
}
