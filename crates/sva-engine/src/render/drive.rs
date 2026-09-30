// Concern: pulls a table block by block until its range ends or `until` holds, dropping what no later block reads | Non-concern: what a value computes | IO: (Table, range) -> the root's blocks

use sva_samples::Extent;

use super::RenderConfig;
use super::table::spill::Spill;
use super::table::{Pulled, Table};
use super::until::{Known, Until};
use crate::cache::Recording;
use crate::error::EngineError;
use crate::flops::Work;
use crate::query::{DEFAULT_FRAME_SECS, Representation};

pub(super) struct Driver {
    pub(super) table: Table,
    pub(super) start: i64,
    pub(super) at: i64,
    last: i64,
    block: usize,
    until: Option<Until>,
    frame: usize,
    stop: Option<i64>,
    end: Option<i64>,
    /// Root samples held behind `at`, for `until` to read its open frame.
    keep: i64,
    most_bytes: usize,
    pub(super) work: Work,
    pub(super) recording: Recording,
    pub(super) spill: Option<Spill>,
    /// A live driver starts silent what is not ready when read, never stalling on it.
    pub(super) live: bool,
    /// Each value it started silent.
    pub(super) dropped: Vec<String>,
}

pub struct Block {
    planes: Vec<Vec<f64>>,
    start: i64,
}

impl Block {
    pub fn start(&self) -> i64 {
        self.start
    }

    pub fn len(&self) -> usize {
        self.planes[0].len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn width(&self) -> usize {
        self.planes.len()
    }

    pub fn plane(&self, c: usize) -> &[f64] {
        &self.planes[c]
    }
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
        table: Table,
        range: Extent,
        block: usize,
        config: &RenderConfig,
        recording: Recording,
    ) -> Driver {
        let frame = frame(config, config.rate);
        Driver {
            table,
            start: range.start,
            at: range.start,
            last: range.end,
            block,
            keep: config.until.as_ref().map_or(0, |_| frame as i64),
            until: config.until.clone(),
            frame,
            stop: None,
            end: None,
            most_bytes: 0,
            work: Work {
                waves: Some(0),
                ..Work::default()
            },
            recording,
            spill: None,
            live: false,
            dropped: Vec::new(),
        }
    }

    pub(super) fn replace(&mut self, table: Table, last: i64) {
        self.table = table;
        self.last = last;
        if self.stop.is_none() {
            self.end = None;
        }
    }

    pub(super) fn last(&self) -> i64 {
        self.last
    }

    pub(super) fn next_to(&self) -> i64 {
        self.last
            .min(self.at.saturating_add(self.block as i64))
            .max(self.at)
    }

    pub(super) fn next_block(&mut self) -> Result<Option<Block>, EngineError> {
        let from = self.at;
        if !self.pull()? {
            return Ok(None);
        }
        let end = self.end.map_or(self.at, |end| end.clamp(from, self.at));
        let held = self.table.samples(self.table.root, Extent::new(from, end));
        Ok(Some(Block {
            planes: held.planes,
            start: from,
        }))
    }

    pub(super) fn pull(&mut self) -> Result<bool, EngineError> {
        let from = self.at;
        if self.end.is_some_and(|end| from >= end) {
            return Ok(false);
        }
        let to = self.next_to();
        self.recording.reach(from);
        let window = Extent::new(from, to);
        if from == self.start {
            let history = self
                .table
                .history(window, self.block as i64, &mut self.recording)?;
            self.priced(&history);
        }
        let ahead = (window, self.last);
        let ahead = self.table.ahead(ahead, self.live, &mut self.recording)?;
        self.priced(&ahead.pulled);
        for at in ahead.dropped {
            self.dropped.push(self.table.values[at].name.clone());
        }
        let asked = self.spill.as_ref().map(|_| self.table.demand(window));
        let pulled = self.table.pull(window, &mut self.recording)?;
        self.priced(&pulled);
        if let (Some(spill), Some(asked)) = (&mut self.spill, asked) {
            spill.take(&self.table, &asked);
        }
        self.work.samples += (to - from) as u64;
        self.at = to;
        self.settle(from, to);
        let future = (to < self.last).then(|| Extent::new(to, self.last));
        let keep = Extent::new(from.min(to.saturating_sub(self.keep)), to);
        self.table.release(future, keep, self.start);
        Ok(true)
    }

    fn priced(&mut self, pulled: &Pulled) {
        self.work.priced_flops += pulled.priced;
        self.work.waves = self.work.waves.map(|held| held + pulled.waves);
        self.most_bytes = self.most_bytes.max(pulled.most_bytes);
    }

    pub(super) fn most_bytes(&self) -> usize {
        self.most_bytes
    }

    /// The one place `until` is checked. A level is known once its frame is whole, so only the
    /// frames the last block left open are read again.
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
        let base = open.max(self.start);
        let heard = self.table.samples(self.table.root, Extent::new(base, to));
        let planes: Vec<&[f64]> = heard.planes.iter().map(Vec::as_slice).collect();
        let rate = self.table.values[self.table.root].grid.rate;
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
