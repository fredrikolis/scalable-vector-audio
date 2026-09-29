// Concern: turns one sampled node into the renderer that writes its buffer, or refuses a read no sample holds | Non-concern: collapsing a closed form (mod.rs), the op array | IO: (NodeId) -> a Buffer

use sva_formula::{NodeId, Var};
use sva_samples::machine::ops::Layout;
use sva_samples::{
    Audible, Binary, BufId, Buffer, Ctx, Extent, Formula, Label, Map, NodeRenderer, SampleError,
    Site, SiteId, Slot, Unary, Window, stft, truncate_spectral_sum, truncate_written,
};

use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::render::Render;
use crate::render::extent::Supports;
use crate::typing::{Typing, Value, When};

/// A sampled node is the dearest kind this engine runs, so it is keyed as a collapse is: off
/// the node's identity, the grid it was stepped on and the extent it was stepped over. Its
/// length rides along.
pub fn key(held: &Render, id: NodeId) -> Result<(sva_formula::Hash, usize), EngineError> {
    key_over(held, id, held.extents.of(id))
}

pub(super) fn key_over(
    held: &Render,
    id: NodeId,
    extent: Extent,
) -> Result<(sva_formula::Hash, usize), EngineError> {
    let key = crate::cache::buffer_key(
        held.identity(id)?,
        held.rate(),
        extent,
        held.tys.ty(id).width as usize,
        sva_samples::AliasScore::NotAsked,
    );
    Ok((held.keyed(key), extent.len()))
}

pub fn run(
    held: &mut Render,
    id: NodeId,
    key: Option<sva_formula::Hash>,
    cache: Option<&crate::cache::Lens>,
) -> Result<(), EngineError> {
    let (buffer, label) = stepped(held, id)?;
    if let Some(key) = key {
        super::store(key, &buffer, &label, cache);
    }
    held.buffers.insert(id, buffer);
    held.labels.insert(id, label);
    Ok(())
}

/// One node, one program: every sampled operand it reads is already a buffer of its own.
fn stepped(held: &Render, id: NodeId) -> Result<(Buffer, Label), EngineError> {
    if let Value::Cast(Cast::Istft, source) = *held.tys.value(id) {
        let frames = held
            .frames
            .get(&source)
            .ok_or_else(|| missing(held, source))?;
        let (inverse, label) = stft::inverse(frames, &held.config.profile);
        return Ok((inverse.over(held.extents.of(id), inverse.extent()), label));
    }
    let program = program(held, id)?;
    let buffer = program.writes(held, id, &program.renderer)?;
    Ok((
        buffer,
        Label::measured(held.config.profile.name, held.rate()),
    ))
}

/// One node's program, the slots it reads through and the node behind each slot.
pub(super) struct Program {
    pub renderer: NodeRenderer,
    pub reads: Vec<NodeId>,
    pub layout: Layout,
}

pub(super) fn program(held: &Render, id: NodeId) -> Result<Program, EngineError> {
    program_reading(held, id, &|r| held.buffers.get(&r).map_or(1, |b| b.width))
}

/// `width_of` answers how wide each node this one reads is held.
pub(super) fn program_reading(
    held: &Render,
    id: NodeId,
    width_of: &dyn Fn(NodeId) -> usize,
) -> Result<Program, EngineError> {
    let mut build = Build {
        held,
        supports: Supports::new(held),
        owner: id,
        reads: Vec::new(),
        sites: Vec::new(),
    };
    let renderer = build.of(id)?;
    let (reads, sites) = (build.reads, build.sites);
    let layout = Layout {
        width: held.tys.ty(id).width as usize,
        read_widths: reads.iter().map(|r| width_of(*r)).collect(),
        sites,
    };
    Ok(Program {
        renderer,
        reads,
        layout,
    })
}

impl Program {
    /// What one shape of this program writes over the node's own extent and slots, which is
    /// how a share runs it again with every slot but one silenced.
    pub(super) fn writes(
        &self,
        held: &Render,
        id: NodeId,
        renderer: &NodeRenderer,
    ) -> Result<Buffer, EngineError> {
        let extent = held.extents.of(id);
        let buffers: Vec<Window> = self
            .reads
            .iter()
            .map(|r| {
                let buffer = held.buffers.get(r).expect("a read is materialized first");
                Window::of(buffer, held.extents.support(*r))
            })
            .collect();
        let live: Vec<Extent> = self
            .reads
            .iter()
            .map(|r| {
                let buffer = &held.buffers[r];
                live(&buffer.planes, buffer.start, held.extents.support(*r))
            })
            .collect();
        let ctx = Ctx {
            rate: held.rate(),
            start: extent.start,
            len: extent.len(),
            reads: &buffers,
        };
        renderer
            .run_live(&self.layout, &ctx, &live)
            .map_err(|e| refused(held, id, &e))
    }

    /// What the program runs over `extent`, span by span without the reads dead there, each
    /// read taken as live over its whole support.
    pub(crate) fn priced(&self, held: &Render, extent: Extent) -> Option<u128> {
        let live: Vec<Extent> = self
            .reads
            .iter()
            .map(|r| {
                held.extents
                    .support
                    .get(r)
                    .copied()
                    .unwrap_or(Extent::EVERYWHERE)
            })
            .collect();
        let spans = self
            .renderer
            .spans(&self.layout, (extent.start, extent.end), &live)
            .ok()?;
        spans.iter().try_fold(0u128, |sum, span| {
            let ops = span.renderer.ops(&self.layout).ok()? as u128;
            Some(sum + ops * (span.to - span.from) as u128)
        })
    }
}

/// Where a read can answer anything but +0.0: a held sample that is not +0.0, and any of
/// its support the buffer does not hold, which no value answers.
pub(super) fn live(planes: &[Vec<f64>], start: i64, support: Extent) -> Extent {
    let held = Extent::new(start, start + planes.first().map_or(0, Vec::len) as i64);
    let nonzero = |n: &usize| planes.iter().any(|p| p[*n].to_bits() != 0);
    let inner = match (0..held.len()).find(nonzero) {
        None => Extent::NOWHERE,
        Some(first) => {
            let last = (0..held.len()).rev().find(nonzero).unwrap_or(first);
            Extent::new(held.start + first as i64, held.start + last as i64 + 1)
        }
    };
    let before = support.intersect(Extent::new(i64::MIN, held.start));
    let after = support.intersect(Extent::new(held.end, i64::MAX));
    inner.hull(before).hull(after)
}

struct Build<'a> {
    held: &'a Render,
    supports: Supports<'a>,
    owner: NodeId,
    reads: Vec<NodeId>,
    sites: Vec<Site>,
}

impl Build<'_> {
    fn of(&mut self, id: NodeId) -> Result<NodeRenderer, EngineError> {
        match self.held.tys.value(id).clone() {
            Value::ClosedForm(_) => {
                closed_renderer(&self.held.tys, id).ok_or_else(|| uncollapsed(self.held, id))
            }
            Value::Cast(Cast::Sample, source) => Ok(self.buffer(source, Map::shift(0))),
            Value::Cast(..) => Err(uncollapsed(self.held, id)),
            Value::Read { source, at, .. } => self.read(source, at),
            Value::SelfAt { at } => self.own(at),
            Value::Noise(seed) => Ok(NodeRenderer::Noise(seed)),
            Value::Solver { .. } if id != self.owner => Ok(self.buffer(id, Map::shift(0))),
            Value::Solver { params, varying } => {
                let mut args = Vec::new();
                for (key, _) in params.varying() {
                    args.push(match varying.iter().find(|(k, _)| k == key) {
                        Some((_, arg)) => self.of(*arg)?,
                        None => NodeRenderer::Const(
                            crate::lower::value_of(&params, key).expect("a varying field"),
                        ),
                    });
                }
                let site = self.site(Site::Physics(Box::new(params.structural())));
                let from = self.state_start(id);
                Ok(NodeRenderer::Physics { site, from, args })
            }
            Value::Filter {
                shape,
                x,
                cutoff,
                q,
                gain,
            } => {
                let site = self.site(Site::Filter(shape));
                Ok(NodeRenderer::Filter {
                    site,
                    from: self.state_start(id),
                    x: Box::new(self.of(x)?),
                    cutoff: Box::new(self.of(cutoff)?),
                    q: Box::new(self.of(q)?),
                    gain: Box::new(self.of(gain)?),
                })
            }
            Value::Op { name, args } => self.operation(id, &name, &args),
        }
    }

    /// A stored sample where each of this program's instants reads one; a closed form exactly
    /// at any instant; anything else refused, as no sample holds it.
    fn read(&mut self, source: NodeId, at: When) -> Result<NodeRenderer, EngineError> {
        if let Some(map) = at.map(self.held.rate()) {
            return Ok(self.buffer(source, map));
        }
        let time = match at {
            When::Time(time) => {
                let line = NodeRenderer::Mul(vec![
                    NodeRenderer::Const(time.scale.to_f64()),
                    NodeRenderer::Time,
                ]);
                NodeRenderer::Add(vec![line, NodeRenderer::Const(time.shift.to_f64())])
            }
            When::Moving(time) => self.of(time)?,
            When::Index(index) => return Err(unreadable_index(self.held, self.owner, index)),
        };
        match formula(self.held, source)? {
            Some(formula) => Ok(NodeRenderer::Formula {
                formula,
                width: self.held.tys.ty(source).width as usize,
                time: Box::new(time),
            }),
            None => Err(off_grid(self.held, self.owner, source, at)),
        }
    }

    /// A loop's own past, at a whole sample before the one being written.
    fn own(&mut self, at: When) -> Result<NodeRenderer, EngineError> {
        match at.map(self.held.rate()) {
            Some(map) => Ok(NodeRenderer::Read {
                slot: Slot::Own,
                map,
            }),
            None => match at {
                When::Index(index) => Err(unreadable_index(self.held, self.owner, index)),
                _ => Err(off_grid(self.held, self.owner, self.owner, at)),
            },
        }
    }

    /// A sampled operand is already a buffer, so a renderer reads it rather than recomputing it.
    fn buffer(&mut self, source: NodeId, map: Map) -> NodeRenderer {
        let id = BufId(self.reads.len() as u32);
        self.reads.push(source);
        NodeRenderer::Read {
            slot: Slot::Read(id),
            map,
        }
    }

    /// Where call site `id`'s state starts: its own support's start, never where the run
    /// reading it starts.
    fn state_start(&self, id: NodeId) -> i64 {
        self.supports
            .state_start(id, self.owner)
            .unwrap_or(i64::MIN)
    }

    fn site(&mut self, site: Site) -> SiteId {
        self.sites.push(site);
        SiteId((self.sites.len() - 1) as u32)
    }

    fn operation(
        &mut self,
        id: NodeId,
        name: &str,
        args: &[NodeId],
    ) -> Result<NodeRenderer, EngineError> {
        let mut lowered = Vec::with_capacity(args.len());
        for arg in args {
            lowered.push(self.of(*arg)?);
        }
        let pair = |mut set: Vec<NodeRenderer>| {
            let right = set.pop().expect("two operands");
            let left = set.pop().expect("two operands");
            (Box::new(left), Box::new(right))
        };
        let unary =
            |op: Unary, mut set: Vec<NodeRenderer>| NodeRenderer::Map(op, Box::new(set.remove(0)));
        Ok(match name {
            "+" => NodeRenderer::Add(lowered),
            "*" => NodeRenderer::Mul(lowered),
            "-" => {
                let (l, r) = pair(lowered);
                NodeRenderer::Sub(l, r)
            }
            "/" => {
                let (l, r) = pair(lowered);
                NodeRenderer::Div(l, r)
            }
            "pow" => {
                let (l, r) = pair(lowered);
                NodeRenderer::Pow(l, r)
            }
            "%" => {
                let (l, r) = pair(lowered);
                NodeRenderer::Zip(Binary::Mod, l, r)
            }
            "max" => {
                let (l, r) = pair(lowered);
                NodeRenderer::Zip(Binary::Max, l, r)
            }
            "min" => {
                let (l, r) = pair(lowered);
                NodeRenderer::Zip(Binary::Min, l, r)
            }
            "crop" => {
                let (a, b) = (constant(&lowered, 1), constant(&lowered, 2));
                let shoulder = |at| match lowered.len() > at {
                    true => constant(&lowered, at),
                    false => 0.0,
                };
                let (rise, fall) = (shoulder(3), shoulder(4));
                NodeRenderer::Crop {
                    x: Box::new(lowered.remove(0)),
                    a,
                    b,
                    rise,
                    fall,
                }
            }
            "join" => NodeRenderer::Join(lowered),
            "ch" => {
                let k = constant(&lowered, 1).max(0.0) as usize;
                NodeRenderer::Channel {
                    x: Box::new(lowered.remove(0)),
                    k,
                }
            }
            _ => match sva_formula::Unary::from_name(name) {
                Some(op) => unary(op.into(), lowered),
                None => return Err(unplanned(self.held, id, name)),
            },
        })
    }
}

/// A closed-form node as the machine evaluates it each sample.
pub(super) fn closed_renderer(tys: &Typing, id: NodeId) -> Option<NodeRenderer> {
    let Value::ClosedForm(form) = tys.value(id) else {
        return None;
    };
    match &form.body {
        sva_formula::Body::Const(c) if c.im == 0.0 => Some(NodeRenderer::Const(c.re)),
        body => match crate::lower::constant_value(body, form.var) {
            Some(v) => Some(NodeRenderer::Const(v)),
            None => crate::lower::inline::renderer(tys, id),
        },
    }
}

/// What a read of `source` evaluates at any instant: the noise, `source` as a closed form in
/// `t`, or what a `sample(...)` of one holds, truncated to the band once; `None` where it holds
/// state.
fn formula(held: &Render, source: NodeId) -> Result<Option<Formula>, EngineError> {
    if !crate::schedule::anywhere(&held.tys, source) {
        return Ok(None);
    }
    let form = match held.tys.value(source) {
        Value::Noise(seed) => {
            return Ok(Some(Formula::Drawn {
                seed: *seed,
                rate: held.rate(),
            }));
        }
        Value::Cast(Cast::Sample, of) => *of,
        _ => source,
    };
    let band = Audible::of(&held.config.profile, held.rate());
    let truncated = |e: &sva_samples::CollapseError| super::pointwise::refused(held, form, e);
    if let Ok(sum) = crate::refs::spectral_sum_of(&held.tys, form, Var::T) {
        let sum = truncate_spectral_sum(&sum, band).map_err(|e| truncated(&e))?;
        return Ok(Some(Formula::Sum(Box::new(sum))));
    }
    match crate::refs::substituted_closed_form(&held.tys, form) {
        Some(written) if written.var == Var::T => {
            let body = truncate_written(&written.body, band).map_err(|e| truncated(&e))?;
            Ok(Some(Formula::Written(Box::new(body))))
        }
        _ => Ok(None),
    }
}

/// A node that holds state has a value only at the samples it steps through, at the rate the
/// render asked for.
fn off_grid(held: &Render, owner: NodeId, source: NodeId, at: When) -> EngineError {
    let rate = held.rate();
    let (what, help) = match at {
        When::Time(time) if time.scale == crate::time::Q::ONE => (
            format!(
                "at {} samples from its own instant, between two of its samples at {rate} Hz",
                time.shift.mul(crate::time::Q::int(i64::from(rate))).map_or(
                    "a count past what this engine holds".to_string(),
                    |n| format!("{}", n.to_f64())
                )
            ),
            "read the nearest sample by index, as x[idx(t - d)], or shift by whole sp",
        ),
        When::Time(_) => (
            "at a scaled time, which lands between its samples".to_string(),
            "read it at t plus a whole number of sp, or by index, as x[idx(2*t)]",
        ),
        _ => (
            "at a time that moves, which lands between its samples".to_string(),
            "read it by index, as x[idx(...)], or write what it reads as a closed form",
        ),
    };
    EngineError::refused(Diagnostic {
        code: "render.off_grid_read".to_string(),
        message: format!(
            "`{}` has a value only at the samples it steps through, and `{}` reads it {what}",
            held.tys.name(source),
            held.tys.name(owner)
        ),
        location: Located::at(held.tys.name(owner), None),
        help: help.to_string(),
    })
}

fn unreadable_index(
    held: &Render,
    owner: NodeId,
    index: Option<crate::index::Index>,
) -> EngineError {
    let why = match index {
        Some(_) => "a sample index this far out has no map a machine can read",
        None => {
            "this engine reads an index only as one idx(...) of a line in t, negated or not, \
             plus a count"
        }
    };
    collapse_refused(&held.tys, owner, why, "engine.unreadable_index")
}

pub fn refused(held: &Render, id: NodeId, e: &SampleError) -> EngineError {
    let mut refusal = collapse_refused(&held.tys, id, &e.to_string(), e.code());
    if let (SampleError::ArgumentOutOfRange { .. }, EngineError::Refused(d)) = (e, &mut refusal) {
        d.help = "keep the parameter in its range at every sample; `sva-cli builtins` names \
                  each argument's range"
            .to_string();
    }
    refusal
}

fn collapse_refused(tys: &Typing, id: NodeId, message: &str, code: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message: message.to_string(),
        location: Located::at(tys.name(id), None),
        help: "write the node so the grid can hold it".to_string(),
    })
}

fn uncollapsed(held: &Render, id: NodeId) -> EngineError {
    collapse_refused(
        &held.tys,
        id,
        "a closed form reaches the grid with no collapse written for it",
        "type.samples_in_closed_form",
    )
}

fn missing(held: &Render, id: NodeId) -> EngineError {
    collapse_refused(
        &held.tys,
        id,
        "the frames this reads were never built",
        "cast.istft_needs_frames",
    )
}

fn unplanned(held: &Render, id: NodeId, name: &str) -> EngineError {
    collapse_refused(
        &held.tys,
        id,
        &format!("`{name}` has no sampled form"),
        "engine.unshadered_operation",
    )
}

/// A window's edges are numbers by the time a renderer reads them; lowering refuses anything
/// that moves, so a renderer that holds one here is a hole in that check.
fn constant(lowered: &[NodeRenderer], at: usize) -> f64 {
    match lowered.get(at) {
        Some(NodeRenderer::Const(v)) => *v,
        other => unreachable!("a window edge reached the shader as {other:?}"),
    }
}
