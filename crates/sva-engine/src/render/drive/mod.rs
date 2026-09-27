// Concern: pulls a target's nodes block by block until its range ends or `until` holds | Non-concern: what one node computes, bindings and checkpoints (stream.rs) | IO: (&Render) -> the root's blocks

pub(super) mod node;

use sva_samples::{Extent, Tape};

use super::until::{Known, Until};
use super::{Render, RenderConfig};
use crate::error::EngineError;
use crate::flops::Work;
use crate::query::{DEFAULT_FRAME_SECS, Representation};
use node::Driven;

/// A stream pulls it for as long as its consumer does, and a whole render to its end.
pub(super) struct Driver {
    pub(super) nodes: Vec<Driven>,
    /// `None` where only the nodes under it are pulled.
    pub(super) root: Option<usize>,
    start: i64,
    pub(super) at: i64,
    last: i64,
    block: usize,
    until: Option<Until>,
    frame: usize,
    stop: Option<i64>,
    end: Option<i64>,
    pub(super) work: Work,
}

pub struct Block {
    planes: Vec<Vec<f64>>,
    start: i64,
}

impl Block {
    fn of(tape: &Tape, support: Extent, start: i64, end: i64) -> Block {
        let window = tape.within(support);
        let planes = (0..tape.width())
            .map(|c| (start..end).map(|n| window.at(c, n)).collect())
            .collect();
        Block { planes, start }
    }

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

/// The `envelope` reading's own frame, where one is asked.
pub(super) fn frame(config: &RenderConfig) -> usize {
    let secs = config
        .asks
        .iter()
        .find_map(|ask| match ask.representation {
            Representation::Envelope { frame_secs } => Some(frame_secs),
            _ => None,
        })
        .flatten()
        .unwrap_or(DEFAULT_FRAME_SECS);
    ((secs * f64::from(config.rate)).round() as usize).max(1)
}

impl Driver {
    pub(super) fn new(
        nodes: Vec<Driven>,
        root: Option<usize>,
        range: Extent,
        at: i64,
        block: usize,
        until: Option<Until>,
        config: &RenderConfig,
    ) -> Driver {
        Driver {
            nodes,
            root,
            start: range.start,
            at,
            last: range.end,
            block,
            until,
            frame: frame(config),
            stop: None,
            end: None,
            work: Work {
                waves: Some(0),
                ..Work::default()
            },
        }
    }

    pub(super) fn next_block(&mut self, shell: &Render) -> Result<Option<Block>, EngineError> {
        let from = self.at;
        if !self.pull(shell)? {
            return Ok(None);
        }
        let end = self.end.map_or(self.at, |end| end.clamp(from, self.at));
        let root = &self.nodes[self.root.expect("a stream reads its root")];
        Ok(Some(Block::of(&root.tape, root.support, from, end)))
    }

    /// `false` once the target has ended.
    pub(super) fn pull(&mut self, shell: &Render) -> Result<bool, EngineError> {
        let from = self.at;
        if self.end.is_some_and(|end| from >= end) {
            return Ok(false);
        }
        let to = self
            .last
            .min(from.saturating_add(self.block as i64))
            .max(from);
        for n in 0..self.nodes.len() {
            let (done, rest) = self.nodes.split_at_mut(n);
            rest[0].run(shell, done, from, to)?;
            let (priced, waves) = rest[0].work(from, to);
            self.work.priced_flops += priced;
            self.work.waves = self.work.waves.zip(waves).map(|(held, more)| held + more);
        }
        self.at = to;
        self.work.samples += (to - from) as u64;
        self.settle(shell, from, to);
        Ok(true)
    }

    /// The one place `until` is checked. A level is known once its frame is whole, so only the
    /// frames the last block left open are read again.
    fn settle(&mut self, shell: &Render, from: i64, to: i64) {
        if self.end.is_some() {
            return;
        }
        if self.last <= to {
            self.end = Some(to);
        }
        let (Some(until), Some(root)) = (&self.until, self.root) else {
            return;
        };
        let root = &self.nodes[root];
        let frame = self.frame as i64;
        let open = self.start + (from - 1 - self.start).div_euclid(frame) * frame;
        let base = open.max(root.tape.base()).max(self.start);
        let heard = Block::of(&root.tape, root.support, base, to);
        let rate = shell.config.rate;
        let known = Known::new(
            heard.plane(0),
            base,
            self.start,
            self.frame,
            rate,
            to == self.last,
        );
        if let Some(at) = until.first(&known, base, to) {
            self.stop = Some(at);
            self.end = Some(at.max(from));
        }
    }

    pub(super) fn end(&self) -> Option<i64> {
        self.end
    }

    /// Where `until` first held, which may be before the block it became known in.
    pub(super) fn stop(&self) -> Option<i64> {
        self.stop
    }
}
