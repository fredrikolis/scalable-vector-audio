// Concern: turns one sampled node into the renderer that writes its buffer | Non-concern: collapsing a closed form (mod.rs), the op array itself (sva-samples) | IO: (NodeId) -> a Buffer

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
use crate::typing::Value;

/// A sampled node is the dearest kind this engine runs, so it reads and writes the same
/// store a collapse does: one key off the node's identity, the rate and the window it was
/// stepped over.
pub fn run(
    held: &mut Render,
    id: NodeId,
    cache: Option<&dyn crate::cache::Cache>,
) -> Result<(), EngineError> {
    let samples = super::length(held, id)?;
    let key = crate::cache::buffer_key(
        crate::refs::identity(&held.tys, id)?,
        held.config.rate,
        held.config.horizon.start_secs,
        samples,
        held.tys.ty(id).width as usize,
        sva_samples::AliasScore::NotAsked,
    );
    if let Some((hit, label)) = super::warm(held, id, key, samples, cache) {
        held.buffers.insert(id, hit);
        held.labels.insert(id, label);
        return Ok(());
    }
    let began = crate::cache::Cost::begun();
    let (buffer, label) = stepped(held, id)?;
    super::store(key, &buffer, &label, began.elapsed(), cache);
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
        return Ok(stft::inverse(frames, &held.config.profile));
    }
    let program = program(held, id)?;
    let buffer = program.writes(held, id, &program.renderer)?;
    Ok((
        buffer,
        Label::measured(held.config.profile.name, held.config.rate),
    ))
}

/// One node's program, the slots it reads through, and the node behind each slot.
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
    /// What one shape of this program writes over the node's own window and slots, which is
    /// how a share runs it again with every slot but one silenced.
    pub(super) fn writes(
        &self,
        held: &Render,
        id: NodeId,
        renderer: &NodeRenderer,
    ) -> Result<Buffer, EngineError> {
        let len = held
            .config
            .horizon
            .len(held.config.rate)
            .map_err(|e| collapse_refused(held, id, &e.to_string(), e.code()))?;
        let buffers: Vec<Window> = self
            .reads
            .iter()
            .map(|r| Window::of(held.buffers.get(r).expect("a read is materialized first")))
            .collect();
        let ctx = Ctx {
            rate: held.config.rate,
            origin_secs: held.config.horizon.start_secs,
            len,
            reads: &buffers,
            self_planes: &[],
            written: 0,
        };
        renderer
            .run(&self.layout, &ctx)
            .map_err(|e| refused(held, id, &e))
    }
}

struct Build<'a> {
    held: &'a Render,
    reads: Vec<NodeId>,
    sites: Vec<Site>,
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
                None => Err(varying(self.held, id)),
            },
            Value::Solver(params) => {
                let site = self.site(Site::Physics(params));
                Ok(NodeRenderer::Physics { site })
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
            .map_err(|count| off_grid(self.held, source, site, count, rate))
    }

    /// A sampled operand is already a buffer, so a renderer reads it rather than recomputing it.
    fn buffer(&mut self, source: NodeId, shift: i64) -> NodeRenderer {
        let id = BufId(self.reads.len() as u32);
        self.reads.push(source);
        NodeRenderer::Buffer { id, shift }
    }

    fn site(&mut self, site: Site) -> SiteId {
        self.sites.push(site);
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
    collapse_refused(held, id, &e.to_string(), e.code())
}

fn collapse_refused(held: &Render, id: NodeId, message: &str, code: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message: message.to_string(),
        location: Located::at(held.tys.name(id), None),
        help: "write the node so the grid can hold it".to_string(),
    })
}

fn uncollapsed(held: &Render, id: NodeId) -> EngineError {
    collapse_refused(
        held,
        id,
        "a closed form reaches the grid with no collapse written for it",
        "type.samples_in_closed_form",
    )
}

fn off_grid(
    held: &Render,
    source: NodeId,
    site: sva_formula::Origin,
    count: f64,
    rate: u32,
) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "ref.fractional_shift_on_samples".to_string(),
        message: format!(
            "`@{}` is samples, and this offset is not a whole one.",
            held.tys.name(source)
        ),
        location: held.tys.locate(site),
        help: format!(
            "{} samples at {rate} Hz; pick a rate or a delay that lands on the grid",
            count.abs()
        ),
    })
}

fn varying(held: &Render, id: NodeId) -> EngineError {
    collapse_refused(
        held,
        id,
        "a delay written as a closed form in t has no whole-sample offset this machine can read",
        "engine.varying_delay",
    )
}

fn missing(held: &Render, id: NodeId) -> EngineError {
    collapse_refused(
        held,
        id,
        "the frames this reads were never built",
        "cast.istft_needs_frames",
    )
}

fn unplanned(held: &Render, id: NodeId, name: &str) -> EngineError {
    collapse_refused(
        held,
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
