// Concern: cuts one renderer into spans, each compiled without the reads and crops zero there | Non-concern: where a read is zero, running a span | IO: (NodeRenderer, supports) -> Spanned

use super::Program;
use super::ops::{Layout, lowered};
use super::renderer::{BufId, Formula, Map, NodeRenderer, Slot};
use crate::collapse::Extent;
use crate::error::SampleError;

#[derive(Clone)]
pub struct Span {
    pub from: i64,
    pub to: i64,
    pub renderer: NodeRenderer,
}

/// A renderer cut into spans each pruned of what is exact zero there: a sample's program
/// depends on its index alone.
#[derive(Clone)]
pub struct Spanned {
    spans: Vec<(Span, Program)>,
    layout: Layout,
}

impl Spanned {
    /// Outside `live[k]` read `k` answers exactly +0.0.
    pub fn new(
        renderer: &NodeRenderer,
        layout: &Layout,
        (from, to): (i64, i64),
        live: &[Extent],
    ) -> Result<Spanned, SampleError> {
        let spans = renderer
            .spans(layout, (from, to), live)?
            .into_iter()
            .map(|span| {
                let program = span.renderer.compile(layout)?;
                Ok((span, program))
            })
            .collect::<Result<_, SampleError>>()?;
        Ok(Spanned {
            spans,
            layout: layout.clone(),
        })
    }

    pub fn spans(&self) -> impl Iterator<Item = &Span> {
        self.spans.iter().map(|(span, _)| span)
    }

    pub(super) fn compiled(&self) -> &[(Span, Program)] {
        &self.spans
    }

    /// A program writing +0 where no span reaches.
    pub(super) fn silent(&self) -> Result<Program, SampleError> {
        let zero = match self.layout.width {
            0 | 1 => NodeRenderer::Const(0.0),
            w => NodeRenderer::Join(vec![NodeRenderer::Const(0.0); w]),
        };
        zero.compile(&self.layout)
    }

    /// One per op over `[from, to)`, span by span, and each formula's own.
    pub fn ops(&self, from: i64, to: i64) -> u128 {
        self.spans
            .iter()
            .map(|(span, program)| {
                let n = (to.min(span.to) - from.max(span.from)).max(0) as u128;
                let formulas: usize = program.formulas.iter().map(Formula::ops).sum();
                n * (program.ops.len() + formulas) as u128
            })
            .sum()
    }
}

impl NodeRenderer {
    /// `[from, to)` cut where the dead reads or shut crops change, each span pruned; any cut
    /// of a run writes the same bits.
    pub fn spans(
        &self,
        layout: &Layout,
        (from, to): (i64, i64),
        live: &[Extent],
    ) -> Result<Vec<Span>, SampleError> {
        let mut leaves = Vec::new();
        buffers(self, &mut leaves);
        let reach = |(id, at): Leaf| at.preimage(live[id.0 as usize]);
        let mut edges = vec![from, to];
        for leaf in &leaves {
            let held = reach(*leaf);
            edges.extend([held.start, held.end]);
        }
        windows(self, &mut |(a, b)| edges.extend([a, b]));
        edges.retain(|e| from <= *e && *e <= to);
        edges.sort_unstable();
        edges.dedup();
        let mut out: Vec<Span> = Vec::new();
        for pair in edges.windows(2) {
            let span = Extent::new(pair[0], pair[1]);
            let dead = |leaf: Leaf| reach(leaf).intersect(span).is_empty();
            let renderer = pruned(self, &Among { span, dead: &dead }, layout)?;
            match out.last_mut() {
                Some(last) if last.renderer == renderer => last.to = span.end,
                _ => out.push(Span {
                    from: span.start,
                    to: span.end,
                    renderer,
                }),
            }
        }
        Ok(out)
    }

    /// Each index read no reach bounds held to the samples up to the one being written, all a
    /// machine stepping beside its source has, where `beside` names that source.
    pub fn stepwise(&self, beside: &dyn Fn(Slot) -> bool) -> NodeRenderer {
        match self {
            NodeRenderer::Indexed {
                slot,
                index,
                reach: None,
            } if beside(*slot) => NodeRenderer::Indexed {
                slot: *slot,
                index: index.clone(),
                reach: Some((i64::MIN, 0)),
            },
            other => {
                rebuilt(other, &mut |p| Ok(p.stepwise(beside))).expect("a rewrite that cannot fail")
            }
        }
    }

    /// One per op, and a formula's own.
    pub fn ops(&self, layout: &Layout) -> Result<usize, SampleError> {
        let (lowered, _) = lowered(self, layout)?;
        let formulas: usize = lowered.formulas.iter().map(|f| f.ops()).sum();
        Ok(lowered.ops.len() + formulas)
    }
}

fn width(r: &NodeRenderer, layout: &Layout) -> Result<usize, SampleError> {
    Ok(lowered(r, layout)?.1)
}

/// A buffer read at a fixed map.
type Leaf = (BufId, Map);

fn buffers(r: &NodeRenderer, out: &mut Vec<Leaf>) {
    if let NodeRenderer::Read {
        slot: Slot::Read(id),
        map,
    } = r
    {
        out.push((*id, *map));
    }
    for part in operands(r) {
        buffers(part, out);
    }
}

/// One span: the reads dead over all of it, and the samples that shut its crops.
struct Among<'a> {
    span: Extent,
    dead: &'a dyn Fn(Leaf) -> bool,
}

impl Among<'_> {
    fn shut(&self, window: (i64, i64)) -> bool {
        let open = Extent::new(window.0.min(window.1), window.1);
        open.intersect(self.span).is_empty()
    }
}

/// Every node over its pruned operands, then each sum without the terms that are exact zero
/// where that keeps its width. A product with a factor exact zero over the span, every other
/// factor holding no state, is exact +0 there whatever those factors hold: its zero factor
/// is outside its support.
fn pruned(r: &NodeRenderer, among: &Among, layout: &Layout) -> Result<NodeRenderer, SampleError> {
    let whole = width(r, layout)?;
    let held = rebuilt(r, &mut |p| pruned(p, among, layout))?;
    let zero = |p: &NodeRenderer| zero(p, among);
    Ok(match held {
        NodeRenderer::Crop { x, window, .. } if among.shut(window) && x.stateless() => zeros(whole),
        NodeRenderer::Mul(parts)
            if parts.iter().any(zero) && parts.iter().all(NodeRenderer::stateless) =>
        {
            zeros(whole)
        }
        NodeRenderer::Add(parts) => {
            let live: Vec<NodeRenderer> =
                parts.iter().filter(|p| !nil(p, among)).cloned().collect();
            match live.is_empty() {
                true if whole == 1 => NodeRenderer::Const(0.0),
                false if width(&NodeRenderer::Add(live.clone()), layout)? == whole => {
                    NodeRenderer::Add(live)
                }
                _ => NodeRenderer::Add(parts),
            }
        }
        NodeRenderer::Sub(a, b) if zero(&b) && width(&a, layout)? == whole => *a,
        other => other,
    })
}

/// Exact +0 in each of `width` components.
fn zeros(width: usize) -> NodeRenderer {
    match width {
        1 => NodeRenderer::Const(0.0),
        w => NodeRenderer::Join(vec![NodeRenderer::Const(0.0); w]),
    }
}

/// Exactly +0.0 at every sample of the span.
fn zero(r: &NodeRenderer, among: &Among) -> bool {
    match r {
        NodeRenderer::Read {
            slot: Slot::Read(id),
            map,
        } => (among.dead)((*id, *map)),
        NodeRenderer::Const(v) => v.to_bits() == 0,
        NodeRenderer::Crop { window, .. } if among.shut(*window) => true,
        NodeRenderer::Crop { x, .. } => zero(x, among),
        NodeRenderer::Add(parts) => parts.iter().all(|p| nil(p, among)),
        NodeRenderer::Join(parts) => parts.iter().all(|p| zero(p, among)),
        NodeRenderer::Sub(a, b) => zero(a, among) && zero(b, among),
        _ => false,
    }
}

/// Exactly +0.0 or -0.0 at every sample of the span, which a sum starting from +0 drops alike.
/// A product runs from 1 in order, so a zero factor zeroes it where the constants before it
/// kept it finite and those after it are finite.
fn nil(r: &NodeRenderer, among: &Among) -> bool {
    match r {
        NodeRenderer::Mul(parts) => match parts.iter().position(|p| nil(p, among)) {
            None => false,
            Some(at) => {
                finite(fold(&parts[..at], 1.0, |a, b| a * b))
                    && parts[at + 1..]
                        .iter()
                        .all(|p| nil(p, among) || finite(constant(p)))
            }
        },
        NodeRenderer::Crop { x, .. } => zero(r, among) || nil(x, among),
        other => zero(other, among),
    }
}

/// Every crop's window of samples under `r`.
fn windows(r: &NodeRenderer, found: &mut dyn FnMut((i64, i64))) {
    if let NodeRenderer::Crop { window, .. } = r {
        found(*window);
    }
    for part in operands(r) {
        windows(part, found);
    }
}

fn finite(value: Option<Vec<f64>>) -> bool {
    value.is_some_and(|v| v.iter().all(|x| x.is_finite()))
}

/// Each component of an operand every sample of which is the same number.
fn constant(r: &NodeRenderer) -> Option<Vec<f64>> {
    match r {
        NodeRenderer::Const(v) => Some(vec![*v]),
        NodeRenderer::Join(set) => set.iter().try_fold(Vec::new(), |mut held, p| {
            held.extend(constant(p)?);
            Some(held)
        }),
        NodeRenderer::Add(set) => fold(set, 0.0, |a, b| a + b),
        NodeRenderer::Mul(set) => fold(set, 1.0, |a, b| a * b),
        _ => None,
    }
}

/// As the machine folds operands, each component from `start`, a mono operand read at every one.
fn fold(set: &[NodeRenderer], start: f64, op: fn(f64, f64) -> f64) -> Option<Vec<f64>> {
    let values = set.iter().map(constant).collect::<Option<Vec<_>>>()?;
    let width = values.iter().map(Vec::len).max().unwrap_or(1);
    Some(
        (0..width)
            .map(|c| {
                values
                    .iter()
                    .fold(start, |acc, v| op(acc, super::part(v, c)))
            })
            .collect(),
    )
}

fn operands(r: &NodeRenderer) -> Vec<&NodeRenderer> {
    match r {
        NodeRenderer::Add(set) | NodeRenderer::Mul(set) | NodeRenderer::Join(set) => {
            set.iter().collect()
        }
        NodeRenderer::Sub(a, b)
        | NodeRenderer::Div(a, b)
        | NodeRenderer::Pow(a, b)
        | NodeRenderer::Zip(_, a, b) => vec![a, b],
        NodeRenderer::Map(_, x)
        | NodeRenderer::Crop { x, .. }
        | NodeRenderer::Channel { x, .. } => vec![x],
        NodeRenderer::Filter {
            x, cutoff, q, gain, ..
        } => vec![x, cutoff, q, gain],
        NodeRenderer::Physics { args, .. } => args.iter().collect(),
        NodeRenderer::Formula { time, .. } => vec![time],
        NodeRenderer::Indexed { index, .. } | NodeRenderer::Instant(index) => {
            index.times().into_iter().map(|t| &**t).collect()
        }
        _ => Vec::new(),
    }
}

type Each<'a> = dyn FnMut(&NodeRenderer) -> Result<NodeRenderer, SampleError> + 'a;

fn rebuilt(r: &NodeRenderer, each: &mut Each) -> Result<NodeRenderer, SampleError> {
    let mut one = |p: &NodeRenderer| each(p).map(Box::new);
    Ok(match r {
        NodeRenderer::Add(set) => {
            NodeRenderer::Add(set.iter().map(&mut *each).collect::<Result<_, _>>()?)
        }
        NodeRenderer::Mul(set) => {
            NodeRenderer::Mul(set.iter().map(&mut *each).collect::<Result<_, _>>()?)
        }
        NodeRenderer::Join(set) => {
            NodeRenderer::Join(set.iter().map(&mut *each).collect::<Result<_, _>>()?)
        }
        NodeRenderer::Sub(a, b) => NodeRenderer::Sub(one(a)?, one(b)?),
        NodeRenderer::Div(a, b) => NodeRenderer::Div(one(a)?, one(b)?),
        NodeRenderer::Pow(a, b) => NodeRenderer::Pow(one(a)?, one(b)?),
        NodeRenderer::Zip(f, a, b) => NodeRenderer::Zip(*f, one(a)?, one(b)?),
        NodeRenderer::Map(f, x) => NodeRenderer::Map(*f, one(x)?),
        NodeRenderer::Channel { x, k } => NodeRenderer::Channel { x: one(x)?, k: *k },
        NodeRenderer::Crop {
            x,
            window,
            a,
            b,
            rise,
            fall,
        } => NodeRenderer::Crop {
            x: one(x)?,
            window: *window,
            a: *a,
            b: *b,
            rise: *rise,
            fall: *fall,
        },
        NodeRenderer::Filter {
            site,
            from,
            x,
            cutoff,
            q,
            gain,
        } => NodeRenderer::Filter {
            site: *site,
            from: *from,
            x: one(x)?,
            cutoff: one(cutoff)?,
            q: one(q)?,
            gain: one(gain)?,
        },
        NodeRenderer::Physics { site, from, args } => NodeRenderer::Physics {
            site: *site,
            from: *from,
            args: args.iter().map(&mut *each).collect::<Result<_, _>>()?,
        },
        NodeRenderer::Formula {
            formula,
            width,
            time,
        } => NodeRenderer::Formula {
            formula: formula.clone(),
            width: *width,
            time: one(time)?,
        },
        NodeRenderer::Indexed { slot, index, reach } => NodeRenderer::Indexed {
            slot: *slot,
            index: index.mapped(&mut |t| one(t))?,
            reach: *reach,
        },
        NodeRenderer::Instant(index) => NodeRenderer::Instant(index.mapped(&mut |t| one(t))?),
        leaf => leaf.clone(),
    })
}
