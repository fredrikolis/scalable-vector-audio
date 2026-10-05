// Concern: pulls or skips a value graph a grid block per thread at a time until its range ends or `until` holds | Non-concern: what a value computes | IO: (ValueGraph, range) -> blocks

use std::num::NonZeroUsize;

use sva_samples::{Buffer, Extent};

use super::RenderConfig;
use super::until::{Known, Until};
use super::value_graph::{Pulled, ValueGraph, blocks_end};
use crate::cache::{Memory, Recording};
use crate::error::EngineError;
use crate::query::{DEFAULT_FRAME_SECS, Representation};

/// Nothing memory answered counts as computed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    pub samples: u64,
    pub computed_samples: u64,
}

pub(super) struct Driver {
    pub(super) value_graph: ValueGraph,
    pub(super) start: i64,
    pub(super) at: i64,
    last: i64,
    block: usize,
    threads: NonZeroUsize,
    until: Option<Until>,
    frame: usize,
    stop: Option<i64>,
    end: Option<i64>,
    /// Root samples held behind `at`, for `until` to read its open frame.
    keep: i64,
    /// Where the samples heard without a break since start from: a skip starts them anew.
    heard: i64,
    output: bool,
    most_bytes: usize,
    pub(super) work: Work,
    pub(super) memory: Memory,
    pub(super) recording: Recording,
}

/// The `envelope` reading's own frame at the render's rate, where one is asked.
pub(super) fn frame(config: &RenderConfig, rate: u32) -> usize {
    let secs = config
        .asks
        .iter()
        .find_map(|ask| match ask.representation {
            Representation::Envelope { frame_secs } => Some(frame_secs),
            _ => None,
        })
        .flatten()
        .unwrap_or(DEFAULT_FRAME_SECS);
    ((secs * f64::from(rate)).round() as usize).max(1)
}

impl Driver {
    pub(super) fn new(
        value_graph: ValueGraph,
        range: Extent,
        block: usize,
        config: &RenderConfig,
        (memory, recording): (Memory, Recording),
    ) -> Driver {
        let frame = frame(config, config.rate);
        Driver {
            value_graph,
            start: range.start,
            at: range.start,
            heard: range.start,
            output: !super::dropped(config),
            last: range.end,
            block,
            threads: config.threads,
            keep: config.until.as_ref().map_or(0, |_| frame as i64),
            until: config.until.clone(),
            frame,
            stop: None,
            end: None,
            most_bytes: 0,
            work: Work::default(),
            memory,
            recording,
        }
    }

    /// Ends at `last`, unless `until` already stopped it.
    pub(super) fn bound(&mut self, last: i64) {
        self.last = last;
        if self.stop.is_none() {
            self.end = None;
        }
    }

    pub(super) fn last(&self) -> i64 {
        self.last
    }

    pub(super) fn next(&self) -> Extent {
        let to = self
            .next_to(self.block)
            .min(blocks_end(self.at, self.threads));
        Extent::new(self.at, to)
    }

    fn next_to(&self, n: usize) -> i64 {
        self.last.min(self.at.saturating_add(n as i64)).max(self.at)
    }

    /// `n` samples from where it stands, cut where its range ends; `None` from there on. A
    /// pull may drop the last one's samples, so each is taken first.
    pub(super) fn read(&mut self, n: usize) -> Result<Option<Buffer>, EngineError> {
        let (from, to) = (self.at, self.next_to(n));
        if !self.pulled(n)? {
            return Ok(None);
        }
        let mut out = self.played(from);
        while self.at < to {
            let at = self.at;
            if !self.pulled((to - at) as usize)? {
                break;
            }
            let more = self.played(at);
            for (held, more) in out.planes.iter_mut().zip(more.planes) {
                held.extend(more);
            }
        }
        Ok(Some(out))
    }

    fn played(&self, from: i64) -> Buffer {
        let end = self.end.map_or(self.at, |end| end.clamp(from, self.at));
        self.value_graph
            .samples(self.value_graph.root, Extent::new(from, end))
    }

    /// Stands at `to`, computing nothing before it; the stateful values it started silent,
    /// each asked from before its run reaches.
    pub(super) fn skip(&mut self, to: i64) -> Result<Vec<usize>, EngineError> {
        if self.end.is_some_and(|end| self.at >= end) {
            return Ok(Vec::new());
        }
        let to = to.min(self.last);
        let silenced = self.value_graph.skipped(Extent::new(to, self.last))?;
        self.recording.reach(to);
        (self.at, self.heard) = (to, to);
        let future = (to < self.last).then(|| Extent::new(to, self.last));
        self.value_graph
            .release(future, Extent::NOWHERE, self.since());
        if self.last <= to {
            self.end = Some(to);
        }
        Ok(silenced)
    }

    pub(super) fn pull(&mut self) -> Result<bool, EngineError> {
        self.pulled(self.block)
    }

    /// Up to `n` samples on, within a grid block per thread; false once it ended.
    pub(super) fn pulled(&mut self, n: usize) -> Result<bool, EngineError> {
        let from = self.at;
        if self.end.is_some_and(|end| from >= end) {
            return Ok(false);
        }
        let to = self.next_to(n).min(blocks_end(from, self.threads));
        self.recording.reach(from);
        let asked_range = Extent::new(from, to);
        if from == self.start {
            let history = self.value_graph.history(
                asked_range,
                self.block as i64,
                (&self.memory, &mut self.recording),
                self.threads,
            )?;
            self.counted(&history);
        }
        let pulled = self.value_graph.pull(
            asked_range,
            (&self.memory, &mut self.recording),
            self.threads,
        )?;
        self.counted(&pulled);
        self.work.samples += (to - from) as u64;
        self.at = to;
        self.settle(from, to);
        let future = (to < self.last).then(|| Extent::new(to, self.last));
        let keep = Extent::new(from.min(to.saturating_sub(self.keep)), to);
        self.value_graph.release(future, keep, self.since());
        Ok(true)
    }

    fn since(&self) -> Option<i64> {
        self.output.then_some(self.start)
    }

    fn counted(&mut self, pulled: &Pulled) {
        self.work.computed_samples += pulled.computed_samples;
        self.most_bytes = self.most_bytes.max(pulled.most_bytes);
    }

    pub(super) fn most_bytes(&self) -> usize {
        self.most_bytes
    }

    /// The one place `until` is checked. A level is known once its frame is whole, so only the
    /// frames the last block left open are read again; a frame a skip cut into is never known.
    fn settle(&mut self, from: i64, to: i64) {
        if self.end.is_some() {
            return;
        }
        if self.last <= to {
            self.end = Some(to);
        }
        let Some(until) = &self.until else {
            return;
        };
        let frame = self.frame as i64;
        let open = self.start + (from - 1 - self.start).div_euclid(frame) * frame;
        let base = open.max(self.heard);
        let heard = self
            .value_graph
            .samples(self.value_graph.root, Extent::new(base, to));
        let planes: Vec<&[f64]> = heard.planes.iter().map(Vec::as_slice).collect();
        let rate = self.value_graph.values[self.value_graph.root].grid.rate;
        let known = Known::new(planes, base, self.start, self.frame, rate, to == self.last);
        if let Some(at) = until.first(&known, base, to) {
            self.stop = Some(at);
            self.end = Some(at.max(from));
        }
    }

    pub(super) fn end(&self) -> Option<i64> {
        self.end
    }

    pub(super) fn stop(&self) -> Option<i64> {
        self.stop
    }
}
