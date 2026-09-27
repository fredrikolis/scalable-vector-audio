// Concern: turns one sampled node into the renderer that writes its buffer, or refuses one no grid holds | Non-concern: collapsing a closed form (mod.rs), the op array | IO: (NodeId) -> a Buffer

use sva_formula::NodeId;
use sva_samples::machine::ops::Layout;
use sva_samples::{
    Binary, BufId, Buffer, Ctx, Label, NodeRenderer, SampleError, Site, SiteId, Unary, Window, stft,
};

use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::loops::Delay;
use crate::offset::Offset;
use crate::render::Render;
use crate::render::extent::Supports;
use crate::typing::{Typing, Value};

/// A sampled node is the dearest kind this engine runs, so it is keyed as a collapse is: off
/// the node's identity, the rate and the extent it was stepped over. Its length rides along.
pub fn key(held: &Render, id: NodeId) -> Result<(sva_formula::Hash, usize), EngineError> {
    let extent = held.extents.of(id);
    let key = crate::cache::buffer_key(
        crate::refs::identity(&held.tys, id)?,
        held.config.rate,
        extent,
        held.tys.ty(id).width as usize,
        sva_samples::AliasScore::NotAsked,
    );
    Ok((key, extent.len()))
}

pub fn run(
    held: &mut Render,
    id: NodeId,
    key: sva_formula::Hash,
    cache: Option<&crate::cache::Lens>,
) -> Result<(), EngineError> {
    let (buffer, label) = stepped(held, id)?;
    super::store(key, &buffer, &label, cache);
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
        Label::measured(held.config.profile.name, held.config.rate),
    ))
}

/// One node's program, the slots it reads through, the node behind each slot and behind each
/// call site.
pub(super) struct Program {
    pub renderer: NodeRenderer,
    pub reads: Vec<NodeId>,
    pub layout: Layout,
    pub site_nodes: Vec<NodeId>,
}

/// A read of samples off the grid, or a delay that moves with `t`, is known from the types
/// and the rate, so it refuses wherever it is written, as typing refuses a loop's fractional
/// step, before a range is decided or a sample computed, dependencies first.
pub(super) fn on_the_grid(tys: &Typing, rate: u32) -> Result<(), EngineError> {
    for id in (0..tys.len()).map(|n| NodeId(n as u32)) {
        match *tys.value(id) {
            Value::Read { source, at, site }
                if matches!(tys.ty(id).held, sva_formula::Held::Sampled) =>
            {
                if let Err(count) = at.steps_at(rate) {
                    return Err(off_grid(tys, source, site, count, rate));
                }
            }
            Value::SelfAt(Delay::Varying) => return Err(varying(tys, id)),
            _ => {}
        }
    }
    Ok(())
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
        supports: Supports::new(&held.tys, held.config.rate),
        owner: id,
        reads: Vec::new(),
        sites: Vec::new(),
        site_nodes: Vec::new(),
    };
    let renderer = build.of(id)?;
    let (reads, sites, site_nodes) = (build.reads, build.sites, build.site_nodes);
    let layout = Layout {
        width: held.tys.ty(id).width as usize,
        read_widths: reads.iter().map(|r| width_of(*r)).collect(),
        sites,
    };
    Ok(Program {
        renderer,
        reads,
        layout,
        site_nodes,
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
        let ctx = Ctx {
            rate: held.config.rate,
            start: extent.start,
            len: extent.len(),
            reads: &buffers,
        };
        renderer
            .run(&self.layout, &ctx)
            .map_err(|e| refused(held, id, &e))
    }
}

struct Build<'a> {
    held: &'a Render,
    supports: Supports<'a>,
    owner: NodeId,
    reads: Vec<NodeId>,
    sites: Vec<Site>,
    site_nodes: Vec<NodeId>,
}

impl Build<'_> {
    fn of(&mut self, id: NodeId) -> Result<NodeRenderer, EngineError> {
        match self.held.tys.value(id).clone() {
            Value::ClosedForm(form) if crate::lower::never(&form.body) => {
                Ok(NodeRenderer::Const(f64::INFINITY))
            }
            Value::ClosedForm(form) => match crate::lower::constant_value(&form.body, form.var) {
                Some(v) => Ok(NodeRenderer::Const(v)),
                None => Err(uncollapsed(self.held, id)),
            },
            Value::Cast(Cast::Sample, source) => Ok(self.buffer(source, 0)),
            Value::Cast(..) => Err(uncollapsed(self.held, id)),
            Value::Read { source, at, site } => {
                let reader = self.held.tys.ty(id).held;
                let steps = self.index_offset(source, at, site)?;
                match crate::refs::resolve(&self.held.tys, source, steps, reader)? {
                    crate::refs::Read::BufferHit { source, shift } => {
                        Ok(self.buffer(source, shift))
                    }
                    crate::refs::Read::IndexOffset { source, steps } => {
                        Ok(self.buffer(source, steps))
                    }
                    _ => Err(uncollapsed(self.held, id)),
                }
            }
            Value::Grid(count) => Ok(NodeRenderer::Const(
                count / f64::from(self.held.config.rate),
            )),
            Value::SelfAt(delay) => match self.delay(delay) {
                Some(steps) => Ok(NodeRenderer::SelfAt { steps }),
                None => Err(varying(&self.held.tys, id)),
            },
            Value::Solver(params) => {
                let site = self.site(Site::Physics(params), id);
                let from = self.state_start(id);
                Ok(NodeRenderer::Physics { site, from })
            }
            Value::Filter {
                shape,
                x,
                cutoff,
                q,
                gain,
            } => {
                let site = self.site(Site::Filter(shape), id);
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

    fn index_offset(
        &self,
        source: NodeId,
        at: Offset,
        site: sva_formula::Origin,
    ) -> Result<i64, EngineError> {
        let rate = self.held.config.rate;
        at.steps_at(rate)
            .map_err(|count| off_grid(&self.held.tys, source, site, count, rate))
    }

    /// A sampled operand is already a buffer, so a renderer reads it rather than recomputing it.
    fn buffer(&mut self, source: NodeId, shift: i64) -> NodeRenderer {
        let id = BufId(self.reads.len() as u32);
        self.reads.push(source);
        NodeRenderer::Buffer { id, shift }
    }

    /// Where call site `id`'s state starts: its own support's start, never where the run
    /// reading it starts.
    fn state_start(&self, id: NodeId) -> i64 {
        self.supports
            .state_start(id, self.owner)
            .unwrap_or(i64::MIN)
    }

    fn site(&mut self, site: Site, id: NodeId) -> SiteId {
        self.sites.push(site);
        self.site_nodes.push(id);
        SiteId((self.sites.len() - 1) as u32)
    }

    fn delay(&self, delay: Delay) -> Option<u32> {
        match delay {
            Delay::Steps(steps) => Some(steps),
            Delay::Secs(secs) => {
                Some(((secs * f64::from(self.held.config.rate)).round() as u32).max(1))
            }
            Delay::Varying => None,
        }
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

pub fn refused(held: &Render, id: NodeId, e: &SampleError) -> EngineError {
    collapse_refused(&held.tys, id, &e.to_string(), e.code())
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

fn off_grid(
    tys: &Typing,
    source: NodeId,
    site: sva_formula::Origin,
    count: f64,
    rate: u32,
) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "ref.fractional_shift_on_samples".to_string(),
        message: format!(
            "`@{}` is samples, and this offset is not a whole one.",
            tys.name(source)
        ),
        location: tys.locate(site),
        help: format!(
            "{} samples at {rate} Hz; pick a rate or a delay that lands on the grid",
            count.abs()
        ),
    })
}

fn varying(tys: &Typing, id: NodeId) -> EngineError {
    collapse_refused(
        tys,
        id,
        "a delay written as a closed form in t has no whole-sample offset this machine can read",
        "engine.varying_delay",
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
