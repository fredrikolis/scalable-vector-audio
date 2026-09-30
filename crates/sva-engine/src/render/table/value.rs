// Concern: one value of the table and the samples it holds, merged, cut and viewed on its own clock | Non-concern: computing them (eval.rs), what is asked (demand.rs) | IO: (segment) -> samples

use sva_formula::{Hash, NodeId, SpectralSum};
use sva_samples::machine::ops::Layout;
use sva_samples::{
    Buffer, Extent, Frames, Grid, Label, Machine, NodeRenderer, Rows, Spanned, Tape, Window,
};

use super::segments::Segments;
use crate::error::{Diagnostic, EngineError, Located};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Key {
    pub(crate) identity: Hash,
    pub(crate) step: (i128, i128),
}

pub(crate) enum Kind {
    Rows(Box<Rows>),
    Program(Box<Program>),
    Frames {
        window: usize,
        hop: usize,
    },
    Istft,
    Spectrum(Box<SpectralSum>),
    /// Stored samples, loaded as asked; a value it reads computes what they miss.
    Stored(std::sync::Arc<crate::cache::Stored>),
}

pub(crate) struct Program {
    pub(crate) renderer: NodeRenderer,
    pub(crate) spanned: Spanned,
    pub(crate) layout: Layout,
    /// Where a stateful value's run starts.
    pub(crate) start: Option<i64>,
    pub(crate) own: i64,
    /// A program that only reads one value at a whole-sample shift is that value moved: the
    /// slot and the shift, and it computes and holds nothing of its own.
    pub(crate) alias: Option<(usize, i64)>,
    pub(crate) machine: Option<Machine>,
    /// Its state where its run passes each of these, kept for the store.
    pub(crate) marks: std::collections::BTreeMap<i64, sva_samples::MachineState>,
}

impl Program {
    pub(crate) fn stateful(&self) -> bool {
        self.start.is_some()
    }
}

pub(crate) enum Held {
    Segments(Vec<Buffer>),
    Run(Tape),
    Frames(Option<Box<Frames>>),
}

pub(crate) struct Value {
    pub(crate) key: Key,
    pub(crate) node: Option<NodeId>,
    pub(crate) name: String,
    pub(crate) grid: Grid,
    pub(crate) width: usize,
    pub(crate) support: Extent,
    pub(crate) period: Option<i64>,
    pub(crate) kind: Kind,
    pub(crate) reads: Vec<usize>,
    pub(crate) held: Held,
    /// Every segment computed, in order.
    pub(crate) evaluated: Vec<Extent>,
    pub(crate) label: Option<Label>,
    /// Where a stateful value's arguments switch, and its identity before each.
    pub(crate) switches: Vec<(i64, Hash)>,
    pub(crate) moved: f64,
    /// Its samples are its identity's alone; one an edit carried on, or anything reading one,
    /// holds a history no key names, so the store neither answers nor keeps it.
    pub(crate) pure: bool,
}

impl Value {
    /// The value it reads and the shift it reads it at, where it only moves that value.
    pub(crate) fn alias(&self) -> Option<(usize, i64)> {
        match &self.kind {
            Kind::Program(program) => program.alias.map(|(slot, by)| (self.reads[slot], by)),
            _ => None,
        }
    }

    /// The value it only moves and by how much: an alias, or a crop with no fade of a read,
    /// whose window holds all it may be nonzero over.
    pub(crate) fn moves(&self) -> Option<(usize, i64)> {
        let Kind::Program(program) = &self.kind else {
            return None;
        };
        if let Some(moved) = self.alias() {
            return Some(moved);
        }
        let NodeRenderer::Crop {
            x,
            window,
            rise,
            fall,
            ..
        } = &program.renderer
        else {
            return None;
        };
        let NodeRenderer::Read {
            slot: sva_samples::Slot::Read(slot),
            map,
        } = x.as_ref()
        else {
            return None;
        };
        let held = Extent::new(window.0, window.1);
        let bare = *rise <= 0.0 && *fall <= 0.0 && map.a == 1 && map.d == 1;
        let within = held.intersect(self.support) == self.support;
        (bare && within && program.start.is_none())
            .then(|| (self.reads[slot.0 as usize], map.at(0)))
    }

    pub(crate) fn covers(&self) -> Segments {
        let mut out = Segments::default();
        if let Kind::Stored(stored) = &self.kind {
            for e in stored.extents() {
                out.add(e.intersect(self.support));
            }
        }
        out
    }

    pub(crate) fn holding(&self) -> Segments {
        let mut out = Segments::default();
        match &self.held {
            Held::Segments(parts) => parts.iter().for_each(|b| out.add(b.extent())),
            Held::Run(tape) => out.add(Extent::new(tape.base(), tape.end())),
            Held::Frames(Some(_)) => out.add(self.support),
            Held::Frames(None) => {}
        }
        out
    }

    pub(crate) fn end(&self) -> Option<i64> {
        match (&self.kind, &self.held) {
            (Kind::Program(program), Held::Run(tape)) if program.machine.is_some() => {
                Some(tape.end())
            }
            _ => None,
        }
    }

    pub(crate) fn bytes(&self) -> usize {
        let planes = |b: &Buffer| b.len() * b.width * size_of::<f64>();
        let held = match &self.held {
            Held::Segments(parts) => parts.iter().map(planes).sum(),
            Held::Run(tape) => tape.capacity() * tape.width() * size_of::<f64>(),
            Held::Frames(Some(frames)) => {
                frames.width * frames.frames * frames.bins * 2 * size_of::<f64>()
            }
            Held::Frames(None) => 0,
        };
        let state = match &self.kind {
            Kind::Program(program) => program.machine.as_ref().map_or(0, Machine::bytes),
            _ => 0,
        };
        held + state
    }

    /// One held segment where one meets `over`, else those it meets laid into one.
    pub(crate) fn window(&self, over: Extent) -> std::borrow::Cow<'_, Buffer> {
        let over = match self.period {
            Some(n) => Extent::new(0, n),
            None => over,
        };
        match &self.held {
            Held::Run(tape) => std::borrow::Cow::Owned(tape.clone().into_buffer(self.grid.rate)),
            Held::Segments(parts) => {
                let meets: Vec<&Buffer> = parts
                    .iter()
                    .filter(|b| !b.extent().intersect(over).is_empty())
                    .collect();
                match meets.as_slice() {
                    [one] => std::borrow::Cow::Borrowed(*one),
                    _ => std::borrow::Cow::Owned(laid(&meets, self.width, self.grid.rate)),
                }
            }
            Held::Frames(_) => {
                std::borrow::Cow::Owned(Buffer::silence(self.grid.rate, self.width, 0))
            }
        }
    }

    pub(crate) fn samples(&self, over: Extent) -> Buffer {
        let mut planes = vec![vec![0.0; over.len()]; self.width.max(1)];
        let mut lay = |window: Window| {
            for (c, plane) in planes.iter_mut().enumerate() {
                for (i, n) in (over.start..over.end).enumerate() {
                    if let Some(v) = window.get(c, n) {
                        plane[i] = v;
                    }
                }
            }
        };
        match &self.held {
            Held::Run(tape) => lay(tape.within(self.support)),
            Held::Segments(parts) => {
                for part in parts {
                    lay(Window::of(part, self.support).folded(self.period));
                }
            }
            Held::Frames(_) => {}
        }
        let mut out = Buffer::of_planes(self.grid.rate, planes);
        out.start = over.start;
        out
    }

    /// A run keeps everything from `kept`'s first sample on.
    pub(crate) fn retain(&mut self, kept: &Segments) {
        match &mut self.held {
            Held::Segments(parts) => {
                let mut out = Vec::new();
                for part in parts.drain(..) {
                    for e in kept.intersect(part.extent()).iter() {
                        out.push(match e == part.extent() {
                            true => part.clone(),
                            false => part.over(e, part.extent()),
                        });
                    }
                }
                *parts = out;
            }
            Held::Run(tape) => {
                let from = kept.iter().next().map_or(tape.end(), |first| first.start);
                tape.forget_before(from);
                if tape.base() == tape.end() {
                    *tape = Tape::new(tape.width(), 0, tape.end());
                }
            }
            Held::Frames(frames) => {
                if kept.is_empty() {
                    *frames = None;
                }
            }
        }
    }

    /// A segment continuing the one before it grows that one in place, amortised O(1) a
    /// sample, as a value computed block by block is; any other overlap is laid anew.
    pub(crate) fn hold(&mut self, buffer: Buffer) {
        let Held::Segments(parts) = &mut self.held else {
            unreachable!("only a value with no state holds segments");
        };
        let at = buffer.extent();
        let first = parts.partition_point(|b| b.extent().end < at.start);
        let last = parts.partition_point(|b| b.extent().start <= at.end);
        match &mut parts[first..last] {
            [] => parts.insert(first, buffer),
            [held] if held.extent().end == at.start && held.width == buffer.width => {
                for (plane, more) in held.planes.iter_mut().zip(&buffer.planes) {
                    plane.extend_from_slice(more);
                }
            }
            touching => {
                let mut merged: Vec<&Buffer> = touching.iter().collect();
                merged.push(&buffer);
                let joined = laid(&merged, self.width, self.grid.rate);
                parts.splice(first..last, [joined]);
            }
        }
    }
}

fn laid(parts: &[&Buffer], width: usize, rate: u32) -> Buffer {
    let hull = parts
        .iter()
        .fold(Extent::NOWHERE, |held, b| held.hull(b.extent()));
    let mut planes = vec![vec![0.0; hull.len()]; width.max(1)];
    for part in parts {
        let at = (part.start - hull.start) as usize;
        for (c, plane) in planes.iter_mut().enumerate() {
            let held = part.plane(c.min(part.width.saturating_sub(1)));
            plane[at..at + held.len()].copy_from_slice(held);
        }
    }
    let mut out = Buffer::of_planes(rate, planes);
    out.start = hull.start;
    out
}

pub(crate) fn finite<'a>(
    name: &str,
    mut samples: impl Iterator<Item = &'a f64>,
) -> Result<(), EngineError> {
    match samples.all(|v| v.is_finite()) {
        true => Ok(()),
        false => Err(EngineError::refused(Diagnostic {
            code: "collapse.not_finite".to_string(),
            message: format!("`{name}` passes the largest double where it is read"),
            location: Located::at(name, None),
            help: "crop a decay at its onset, so it is read only where it falls".to_string(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments(width: usize) -> Value {
        Value {
            key: Key {
                identity: Hash(0, 0),
                step: (1, 1),
            },
            node: None,
            name: "stream".to_string(),
            grid: Grid::of(8_000),
            width,
            support: Extent::EVERYWHERE,
            period: None,
            kind: Kind::Istft,
            reads: Vec::new(),
            held: Held::Segments(Vec::new()),
            evaluated: Vec::new(),
            label: None,
            switches: Vec::new(),
            moved: 0.0,
            pure: true,
        }
    }

    fn block(start: i64, len: usize, width: usize) -> Buffer {
        let mut b = Buffer::of_planes(8_000, vec![vec![start as f64; len]; width]);
        b.start = start;
        b
    }

    /// `hold`'s cost contract, which a render streamed as its own target lives on: held block
    /// by block, the samples already held move a logarithmic number of times, so each sample
    /// costs amortised O(1). Where they move is the one observable of a copy; any geometric
    /// growth passes, and relaying the segment each block fails.
    #[test]
    fn a_value_held_block_by_block_grows_in_place() {
        let (blocks, len) = (1024usize, 64usize);
        let mut value = segments(2);
        let mut moves = 0;
        let mut last = std::ptr::null();
        for k in 0..blocks {
            value.hold(block((k * len) as i64, len, 2));
            let Held::Segments(parts) = &value.held else {
                unreachable!("a value of segments");
            };
            assert_eq!(parts.len(), 1, "one run of contiguous samples");
            let at = parts[0].plane(0).as_ptr();
            moves += usize::from(at != last);
            last = at;
        }
        assert!(
            moves <= 2 * blocks.ilog2() as usize,
            "{moves} moves over {blocks} blocks"
        );
        let Held::Segments(parts) = &value.held else {
            unreachable!("a value of segments");
        };
        assert_eq!(parts[0].extent(), Extent::new(0, (blocks * len) as i64));
        assert_eq!(parts[0].plane(1)[len * 5], (len * 5) as f64);
    }
}
