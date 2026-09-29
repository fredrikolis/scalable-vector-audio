// Concern: turns one sampled node into the renderer that writes its buffer, or refuses one no grid holds | Non-concern: collapsing a closed form (mod.rs), the op array | IO: (NodeId) -> a Buffer

use sva_formula::NodeId;
use sva_samples::machine::ops::Layout;
use sva_samples::{
    At, Binary, BufId, Buffer, Ctx, Extent, Label, Map, NodeRenderer, SampleError, Site, SiteId,
    Slot, Unary, Window, stft,
};

use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::render::Render;
use crate::render::extent::Supports;
use crate::typing::{Typing, Value, When};

/// A sampled node is the dearest kind this engine runs, so it is keyed as a collapse is: off
/// the node's identity, its lattice and the extent it was stepped over. Its length rides along.
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
        held.lattice(),
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
        Label::measured(held.config.profile.name, held.lattice()),
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
    let extent = held.extents.decided.get(&id).copied();
    program_over(held, id, width_of, extent.unwrap_or(Extent::EVERYWHERE))
}

/// Over `extent`, which a loop's kernel is chosen for, before the render holds it.
pub(super) fn program_over(
    held: &Render,
    id: NodeId,
    width_of: &dyn Fn(NodeId) -> usize,
    extent: Extent,
) -> Result<Program, EngineError> {
    built(held, id, width_of, (extent, held.lattice(), false))
}

/// A node that holds no state and draws no noise, stepped at the output's own instants: each
/// closed form in it read there, and only what it reads of other nodes read between their
/// lattice samples.
pub(super) fn at_output(
    held: &Render,
    id: NodeId,
    width_of: &dyn Fn(NodeId) -> usize,
) -> Result<Option<Program>, EngineError> {
    match held.extents.decided.get(&id) {
        Some(extent) => at_output_over(held, id, width_of, *extent),
        None => Ok(None),
    }
}

/// Before the render holds `extent`, so its reads can be demanded.
pub(super) fn at_output_over(
    held: &Render,
    id: NodeId,
    width_of: &dyn Fn(NodeId) -> usize,
    extent: Extent,
) -> Result<Option<Program>, EngineError> {
    let stepped = held.tys.ty(id).held == sva_formula::Held::Sampled
        && !matches!(held.tys.value(id), Value::Cast(Cast::Istft, _));
    if !stepped || held.on_its_lattice() {
        return Ok(None);
    }
    if !pointwise_safe(&program_over(held, id, width_of, extent)?.renderer) {
        return Ok(None);
    }
    let program = built(held, id, width_of, (extent, held.config.rate, true))?;
    Ok(Some(program))
}

/// Holds no state and draws no lattice noise, so any instant reads it.
fn pointwise_safe(renderer: &NodeRenderer) -> bool {
    let mut noise = false;
    super::extent::leaves(renderer, &mut |leaf| {
        noise |= matches!(leaf, NodeRenderer::Noise(_))
    });
    renderer.stateless() && !noise
}

fn built(
    held: &Render,
    id: NodeId,
    width_of: &dyn Fn(NodeId) -> usize,
    (extent, rate, inlines): (Extent, u32, bool),
) -> Result<Program, EngineError> {
    let mut build = Build {
        held,
        supports: Supports::new(held),
        owner: id,
        rate,
        inlines,
        extent,
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
            rate: held.lattice(),
            start: extent.start,
            len: extent.len(),
            reads: &buffers,
        };
        renderer
            .run_live(&self.layout, &ctx, &live)
            .map_err(|e| refused(held, id, &e))
    }

    /// A program `at_output` builds, run over the output samples `over`.
    pub(super) fn at_output(
        &self,
        held: &Render,
        id: NodeId,
        over: Extent,
    ) -> Result<Buffer, EngineError> {
        let buffers: Vec<Window> = self
            .reads
            .iter()
            .map(|r| Window::of(&held.buffers[r], held.extents.support(*r)))
            .collect();
        let ctx = Ctx {
            rate: held.config.rate,
            start: over.start,
            len: over.len(),
            reads: &buffers,
        };
        self.renderer
            .run(&self.layout, &ctx)
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
    /// The instants it steps at, a second.
    rate: u32,
    /// Whether a node it reads at its own instants is stepped here rather than read.
    inlines: bool,
    extent: Extent,
    reads: Vec<NodeId>,
    sites: Vec<Site>,
}

impl Build<'_> {
    fn of(&mut self, id: NodeId) -> Result<NodeRenderer, EngineError> {
        match self.held.tys.value(id).clone() {
            Value::ClosedForm(_) => {
                closed_renderer(&self.held.tys, id).ok_or_else(|| uncollapsed(self.held, id))
            }
            Value::Cast(Cast::Sample, source) => Ok(self.buffer(source, self.lattice_map())),
            Value::Cast(..) => Err(uncollapsed(self.held, id)),
            Value::Read {
                source,
                at: When::Time(crate::time::Affine::NOW),
                ..
            } if self.inlines => self.inlined(source),
            Value::Read { source, at, .. } => {
                let at = self.at(at, self.held.lattice(), false)?;
                Ok(self.buffer(source, at))
            }
            Value::SelfAt { at, .. } => self.own(id, at),
            Value::Noise(seed) => Ok(NodeRenderer::Noise(seed)),
            Value::Solver { .. } if id != self.owner => Ok(self.buffer(id, self.lattice_map())),
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

    /// Where one reading of a node on `source`'s lattice lands, from this program's.
    /// A time whose rationals pass one map is read as the time it spells, each sample.
    fn at(&mut self, at: When, source: u32, by_delay: bool) -> Result<At, EngineError> {
        let reader = f64::from(self.rate);
        let per_sec = f64::from(source);
        match at {
            When::Time(time) => Ok(match time.map(self.rate, source) {
                Some(map) => At::Map(map),
                None => {
                    let line = match time.scale == crate::time::Q::ONE {
                        true => NodeRenderer::Time,
                        false => NodeRenderer::Mul(vec![
                            NodeRenderer::Const(time.scale.to_f64()),
                            NodeRenderer::Time,
                        ]),
                    };
                    let time =
                        NodeRenderer::Add(vec![line, NodeRenderer::Const(time.shift.to_f64())]);
                    At::moving(per_sec, reader, time, by_delay)
                }
            }),
            When::Moving(time) => Ok(At::moving(per_sec, reader, self.of(time)?, by_delay)),
            When::Index(index) => at.map(self.rate, source).map(At::Map).ok_or_else(|| {
                let why = match index {
                    Some(_) => "a lattice index this far out has no map a machine can read",
                    None => {
                        "this engine reads an index only as one idx(...) of a line in t, \
                         negated or not, plus a count"
                    }
                };
                collapse_refused(&self.held.tys, self.owner, why, "engine.unreadable_index")
            }),
        }
    }

    /// A node read where it stands, as its own program where that holds no state and draws no
    /// noise, else its buffer.
    fn inlined(&mut self, source: NodeId) -> Result<NodeRenderer, EngineError> {
        let (reads, sites) = (self.reads.len(), self.sites.len());
        if self.held.tys.ty(source).held == sva_formula::Held::Sampled
            && let Ok(inline) = self.of(source)
            && pointwise_safe(&inline)
        {
            return Ok(inline);
        }
        self.reads.truncate(reads);
        self.sites.truncate(sites);
        let at = self.at(
            When::Time(crate::time::Affine::NOW),
            self.held.lattice(),
            false,
        )?;
        Ok(self.buffer(source, at))
    }

    /// A lattice sample at each of this program's instants.
    fn lattice_map(&self) -> At {
        let (rate, lattice) = (i128::from(self.rate), i128::from(self.held.lattice()));
        At::Map(Map::new(lattice, 0, rate).expect("two rates make one map"))
    }

    /// A loop's own past through the kernel its bound chose, every tap before the sample
    /// being written; off its lattice a loop is never stepped, holding state.
    fn own(&mut self, id: NodeId, at: When) -> Result<NodeRenderer, EngineError> {
        let at = self.at(at, self.held.lattice(), true)?;
        let half_width = match &at {
            At::Map(map) if map.whole() => 0,
            _ if self.rate != self.held.lattice() => 0,
            _ => {
                self.held
                    .loop_kernel(id, self.extent)?
                    .expect("a loop reading between samples has a kernel")
                    .half_width
            }
        };
        Ok(NodeRenderer::Read {
            slot: Slot::Own,
            at,
            half_width,
        })
    }

    /// A sampled operand is already a buffer, so a renderer reads it rather than recomputing it.
    fn buffer(&mut self, source: NodeId, at: At) -> NodeRenderer {
        let id = BufId(self.reads.len() as u32);
        self.reads.push(source);
        let half_width = match &at {
            At::Map(map) if map.whole() => 0,
            _ => sva_samples::plain().half_width(),
        };
        NodeRenderer::Read {
            slot: Slot::Read(id),
            at,
            half_width,
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
