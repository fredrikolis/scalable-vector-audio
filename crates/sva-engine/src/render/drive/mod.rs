// Concern: pulls a target's nodes block by block until its range ends or `until` holds, dropping what ended | Non-concern: what one node computes, what an edit keeps | IO: (&Render) -> the root's blocks

pub(super) mod edit;
pub(super) mod node;

use sva_samples::{Extent, Standing, Tape};

use super::until::{Known, Until};
use super::{Lenses, Render, RenderConfig};
use crate::error::EngineError;
use crate::flops::Work;
use crate::query::{DEFAULT_FRAME_SECS, Representation};
use node::Driven;

/// A stream pulls it for as long as its consumer does, and a whole render to its end.
pub(super) struct Driver {
    pub(super) nodes: Vec<Driven>,
    /// `None` where only the nodes under it are pulled.
    pub(super) root: Option<usize>,
    pub(super) start: i64,
    pub(super) at: i64,
    last: i64,
    block: usize,
    until: Option<Until>,
    frame: usize,
    stop: Option<i64>,
    end: Option<i64>,
    /// Each node past its extent and its readers' reach is dropped.
    prunes: bool,
    pub(super) work: Work,
    /// Where every node stood at one instant an edit may take effect at, and that instant.
    mark: Option<(i64, Vec<Option<Stood>>)>,
}

/// A node's clock's end, and where its machine stood there.
type Stood = (i64, Option<Standing>);

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

    pub(in crate::render) fn of_planes(planes: Vec<Vec<f64>>, start: i64) -> Block {
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

    fn slices(&self) -> Vec<&[f64]> {
        self.planes.iter().map(Vec::as_slice).collect()
    }
}

/// The `envelope` reading's own frame on the render's lattice, where one is asked.
pub(super) fn frame(config: &RenderConfig, lattice: u32) -> usize {
    let secs = config
        .asks
        .iter()
        .find_map(|ask| match ask.representation {
            Representation::Envelope { frame_secs } => Some(frame_secs),
            _ => None,
        })
        .flatten()
        .unwrap_or(DEFAULT_FRAME_SECS);
    ((secs * f64::from(lattice)).round() as usize).max(1)
}

impl Driver {
    pub(super) fn new(
        nodes: Vec<Driven>,
        root: Option<usize>,
        range: Extent,
        block: usize,
        until: Option<Until>,
        shell: &Render,
        prunes: bool,
    ) -> Driver {
        Driver {
            nodes,
            root,
            start: range.start,
            at: range.start,
            last: range.end,
            block,
            until,
            frame: frame(&shell.config, shell.lattice()),
            stop: None,
            end: None,
            prunes,
            work: Work {
                waves: Some(0),
                ..Work::default()
            },
            mark: None,
        }
    }

    fn mark(&mut self) {
        let stood = self.nodes.iter().map(Driven::stood).collect();
        self.mark = Some((self.at, stood));
    }

    /// Each node `changed` names as it stood at the mark; `false` where it was not at `at`.
    pub(super) fn rewind(&mut self, at: i64, changed: &dyn Fn(usize) -> bool) -> bool {
        let Some((_, stood)) = self.mark.take().filter(|(marked, _)| *marked == at) else {
            return false;
        };
        for (n, (node, stood)) in self.nodes.iter_mut().zip(stood).enumerate() {
            if let (true, Some(stood)) = (changed(n), stood) {
                node.back_to(stood);
            }
        }
        self.at = at;
        self.end = self.end.filter(|end| *end <= at);
        self.stop = self.stop.filter(|stop| *stop <= at);
        true
    }

    /// An edit's nodes, from where it stands.
    pub(super) fn replace(&mut self, nodes: Vec<Driven>, root: Option<usize>, last: i64) {
        self.mark = None;
        self.nodes = nodes;
        self.root = root;
        self.last = last;
        if self.stop.is_none() {
            self.end = None;
        }
    }

    pub(super) fn next_block(
        &mut self,
        shell: &Render,
        lenses: &Lenses,
    ) -> Result<Option<Block>, EngineError> {
        let from = self.at;
        if !self.pull(shell, lenses)? {
            return Ok(None);
        }
        let end = self.end.map_or(self.at, |end| end.clamp(from, self.at));
        let root = &self.nodes[self.root.expect("a stream reads its root")];
        Ok(Some(Block::of(&root.tape, root.support, from, end)))
    }

    /// `false` once the target has ended.
    pub(super) fn pull(&mut self, shell: &Render, lenses: &Lenses) -> Result<bool, EngineError> {
        self.pulled(shell, lenses, None)
    }

    /// A block, cut at `edit` where it would pass it, and marked there to come back to.
    pub(super) fn pull_marking(
        &mut self,
        shell: &Render,
        lenses: &Lenses,
        edit: i64,
    ) -> Result<bool, EngineError> {
        let ahead = (self.at < edit).then_some(edit);
        let pulled = self.pulled(shell, lenses, ahead)?;
        if pulled && Some(self.at) == ahead {
            self.mark();
        }
        Ok(pulled)
    }

    fn pulled(
        &mut self,
        shell: &Render,
        lenses: &Lenses,
        stop: Option<i64>,
    ) -> Result<bool, EngineError> {
        let from = self.at;
        if self.end.is_some_and(|end| from >= end) {
            return Ok(false);
        }
        let to = self
            .last
            .min(from.saturating_add(self.block as i64))
            .min(stop.unwrap_or(i64::MAX))
            .max(from);
        for n in 0..self.nodes.len() {
            let (done, rest) = self.nodes.split_at_mut(n);
            let lag = rest[0].lag;
            let (ran, until) = rest[0].run(shell, done, from - lag, to - lag)?;
            let (priced, waves) = rest[0].work(ran, until);
            self.work.priced_flops += priced;
            self.work.waves = self.work.waves.zip(waves).map(|(held, more)| held + more);
        }
        self.at = to;
        self.work.samples += (to - from) as u64;
        self.settle(shell, from, to);
        if self.prunes {
            for (at, node) in self.nodes.iter_mut().enumerate() {
                if Some(at) != self.root && node.spent(to - node.lag) {
                    node.end(shell, lenses);
                }
            }
        }
        Ok(true)
    }

    pub(super) fn bytes(&self) -> usize {
        self.nodes.iter().map(Driven::bytes).sum()
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
        let rate = shell.lattice();
        let known = Known::new(
            heard.slices(),
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
