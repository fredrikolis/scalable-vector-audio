// Concern: cuts one renderer into spans, each compiled without the reads and crops zero there | Non-concern: where a read is zero, running a span | IO: (NodeRenderer, supports) -> Spanned

use super::CompiledOps;
use super::ops::{self, Layout, lowered};
use super::renderer::{Formula, Index, NodeRenderer, Slot};
use crate::error::SampleError;
use crate::grid::Extent;

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
    spans: Vec<(Span, CompiledOps)>,
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
                let compiled_ops = span.renderer.compile(layout)?;
                Ok((span, compiled_ops))
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

    pub(super) fn grid(&self) -> crate::grid::Grid {
        self.layout.grid
    }

    pub(super) fn compiled(&self) -> &[(Span, CompiledOps)] {
        &self.spans
    }

    /// A program writing +0 where no span reaches.
    pub(super) fn silent(&self) -> Result<CompiledOps, SampleError> {
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
            .map(|(span, compiled_ops)| {
                let n = (to.min(span.to) - from.max(span.from)).max(0) as u128;
                let formulas: usize = compiled_ops.formulas.iter().map(Formula::ops).sum();
                n * (compiled_ops.ops.len() + formulas) as u128
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
        let mut root = Cut::of(self, layout, live)?;
        let mut edges = vec![from, to];
        root.edges(&mut edges);
        edges.retain(|e| from <= *e && *e <= to);
        edges.sort_unstable();
        edges.dedup();
        let mut out: Vec<Span> = Vec::new();
        for pair in edges.windows(2) {
            let span = Extent::new(pair[0], pair[1]);
            let changed = root.prune(span, layout)?;
            match out.last_mut() {
                Some(last) if !changed => last.to = span.end,
                last => {
                    let renderer = root.made();
                    match last {
                        Some(last) if last.renderer == renderer => last.to = span.end,
                        _ => out.push(Span {
                            from: span.start,
                            to: span.end,
                            renderer,
                        }),
                    }
                }
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
            other => rebuilt(other, &mut |p| p.stepwise(beside)),
        }
    }

    /// One per op, and a formula's own.
    pub fn ops(&self, layout: &Layout) -> Result<usize, SampleError> {
        let (lowered, _) = lowered(self, layout)?;
        let formulas: usize = lowered.formulas.iter().map(|f| f.ops()).sum();
        Ok(lowered.ops.len() + formulas)
    }
}

/// One node of the renderer, its width settled once, and what pruning made of it over the
/// latest span; that holds over every later span ending by `until`, since only the reads
/// and crops under it decide it.
struct Cut<'r> {
    node: &'r NodeRenderer,
    width: usize,
    operands: Vec<Cut<'r>>,
    /// Where a read of another node can be nonzero, in this node's samples.
    reach: Option<Extent>,
    until: i64,
    made: Made,
    held: Held,
}

#[derive(Clone, PartialEq)]
enum Made {
    Kept,
    /// Exact +0 in each component.
    Zeros,
    /// A sum over only these operands.
    Terms(Vec<usize>),
    /// A difference whose subtrahend is exact zero, as its minuend.
    Minuend,
}

/// What the node as made holds over the span.
#[derive(Default)]
struct Held {
    /// Exactly +0.0 at every sample.
    zero: bool,
    /// Exactly +0.0 or -0.0 at every sample, which a sum starting from +0 drops alike.
    nil: bool,
    stateless: bool,
    /// Each component where every sample is the same number.
    constant: Option<Vec<f64>>,
}

impl<'r> Cut<'r> {
    fn of(
        node: &'r NodeRenderer,
        layout: &Layout,
        live: &[Extent],
    ) -> Result<Cut<'r>, SampleError> {
        let operands = node
            .operands()
            .into_iter()
            .map(|p| Cut::of(p, layout, live))
            .collect::<Result<Vec<_>, _>>()?;
        let widths: Vec<usize> = operands.iter().map(|o| o.width).collect();
        let reach = match node {
            NodeRenderer::Read {
                slot: Slot::Read(id),
                map,
            } => Some(map.preimage(live[id.0 as usize])),
            _ => None,
        };
        Ok(Cut {
            node,
            width: ops::width(node, &widths, layout)?,
            operands,
            reach,
            until: i64::MIN,
            made: Made::Kept,
            held: Held::default(),
        })
    }

    fn edges(&self, out: &mut Vec<i64>) {
        if let Some(reach) = self.reach {
            out.extend([reach.start, reach.end]);
        }
        if let NodeRenderer::Crop { window, .. } = self.node {
            out.extend([window.0, window.1]);
        }
        for o in &self.operands {
            o.edges(out);
        }
    }

    /// Re-prunes what `span` passes the `until` of, bottom-up; whether what it makes changed.
    fn prune(&mut self, span: Extent, layout: &Layout) -> Result<bool, SampleError> {
        if span.end <= self.until {
            return Ok(false);
        }
        let mut below = false;
        for o in &mut self.operands {
            below |= o.prune(span, layout)?;
        }
        let mut until = self
            .operands
            .iter()
            .map(|o| o.until)
            .min()
            .unwrap_or(i64::MAX);
        let over = |e: Extent| {
            [e.start, e.end]
                .into_iter()
                .filter(|&x| x > span.start)
                .min()
        };
        let shut = match self.node {
            NodeRenderer::Crop { window, .. } => {
                let open = Extent::new(window.0.min(window.1), window.1);
                until = until.min(over(open).unwrap_or(i64::MAX));
                open.intersect(span).is_empty()
            }
            _ => false,
        };
        let dead = self.reach.is_some_and(|reach| {
            until = until.min(over(reach).unwrap_or(i64::MAX));
            reach.intersect(span).is_empty()
        });
        let made = self.decided(shut, layout)?;
        let changed = self.until == i64::MIN || made != self.made || (below && made != Made::Zeros);
        self.held = self.holding(&made, shut, dead);
        self.made = made;
        self.until = until;
        Ok(changed)
    }

    /// A product with a factor exact zero over the span, every other factor holding no
    /// state, is exact +0 there whatever those factors hold: its zero factor is outside its
    /// support. A sum drops the terms that are exact zero where that keeps its width.
    fn decided(&self, shut: bool, layout: &Layout) -> Result<Made, SampleError> {
        let ops = &self.operands;
        Ok(match self.node {
            NodeRenderer::Crop { .. } if shut && ops[0].held.stateless => Made::Zeros,
            NodeRenderer::Mul(_)
                if ops.iter().any(|o| o.held.zero) && ops.iter().all(|o| o.held.stateless) =>
            {
                Made::Zeros
            }
            NodeRenderer::Add(_) => {
                let live: Vec<usize> = (0..ops.len()).filter(|&k| !ops[k].held.nil).collect();
                let widths: Vec<usize> = live.iter().map(|&k| ops[k].width).collect();
                match live.is_empty() {
                    true if self.width == 1 => Made::Zeros,
                    false if live.len() == ops.len() => Made::Kept,
                    false if ops::width(self.node, &widths, layout)? == self.width => {
                        Made::Terms(live)
                    }
                    _ => Made::Kept,
                }
            }
            NodeRenderer::Sub(..) if ops[1].held.zero && ops[0].width == self.width => {
                Made::Minuend
            }
            _ => Made::Kept,
        })
    }

    fn holding(&self, made: &Made, shut: bool, dead: bool) -> Held {
        let ops = &self.operands;
        let of = |k: usize| &ops[k].held;
        match made {
            Made::Zeros => Held {
                zero: true,
                nil: true,
                stateless: true,
                constant: Some(vec![0.0; self.width]),
            },
            Made::Minuend => Held {
                zero: of(0).zero,
                nil: of(0).nil,
                stateless: of(0).stateless,
                constant: of(0).constant.clone(),
            },
            Made::Terms(live) => {
                let zero = live.iter().all(|&k| of(k).nil);
                Held {
                    zero,
                    nil: zero,
                    stateless: live.iter().all(|&k| of(k).stateless),
                    constant: fold(live.iter().map(|&k| of(k)), 0.0, |a, b| a + b),
                }
            }
            Made::Kept => {
                let all = |f: fn(&Held) -> bool| ops.iter().all(|o| f(&o.held));
                let zero = match self.node {
                    NodeRenderer::Read {
                        slot: Slot::Read(_),
                        ..
                    } => dead,
                    NodeRenderer::Const(v) => v.to_bits() == 0,
                    NodeRenderer::Crop { .. } => shut || of(0).zero,
                    NodeRenderer::Add(_) => all(|h| h.nil),
                    NodeRenderer::Join(_) => all(|h| h.zero),
                    NodeRenderer::Sub(..) => of(0).zero && of(1).zero,
                    _ => false,
                };
                let nil = match self.node {
                    NodeRenderer::Mul(_) => match ops.iter().position(|o| o.held.nil) {
                        None => false,
                        Some(at) => {
                            finite(fold(ops[..at].iter().map(|o| &o.held), 1.0, |a, b| a * b))
                                && ops[at + 1..]
                                    .iter()
                                    .all(|o| o.held.nil || finite(o.held.constant.clone()))
                        }
                    },
                    NodeRenderer::Crop { .. } => zero || of(0).nil,
                    _ => zero,
                };
                let held = ops.iter().map(|o| &o.held);
                let constant = match self.node {
                    NodeRenderer::Const(v) => Some(vec![*v]),
                    NodeRenderer::Join(_) => {
                        held.map(|h| h.constant.clone())
                            .try_fold(Vec::new(), |mut all, c| {
                                all.extend(c?);
                                Some(all)
                            })
                    }
                    NodeRenderer::Add(_) => fold(held, 0.0, |a, b| a + b),
                    NodeRenderer::Mul(_) => fold(held, 1.0, |a, b| a * b),
                    _ => None,
                };
                Held {
                    zero,
                    nil,
                    stateless: !self.node.holds_state() && all(|h| h.stateless),
                    constant,
                }
            }
        }
    }

    fn made(&self) -> NodeRenderer {
        match &self.made {
            Made::Zeros => match self.width {
                1 => NodeRenderer::Const(0.0),
                w => NodeRenderer::Join(vec![NodeRenderer::Const(0.0); w]),
            },
            Made::Minuend => self.operands[0].made(),
            Made::Terms(live) => {
                NodeRenderer::Add(live.iter().map(|&k| self.operands[k].made()).collect())
            }
            Made::Kept => {
                let mut each = self.operands.iter();
                rebuilt(self.node, &mut |_| {
                    each.next().expect("one per operand").made()
                })
            }
        }
    }
}

fn finite(value: Option<Vec<f64>>) -> bool {
    value.is_some_and(|v| v.iter().all(|x| x.is_finite()))
}

/// As the machine folds operands, each component from `start`, a mono operand read at every one.
fn fold<'h>(
    set: impl Iterator<Item = &'h Held>,
    start: f64,
    op: fn(f64, f64) -> f64,
) -> Option<Vec<f64>> {
    let values = set
        .map(|h| h.constant.as_deref())
        .collect::<Option<Vec<_>>>()?;
    let width = values.iter().map(|v| v.len()).max().unwrap_or(1);
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

fn rebuilt(r: &NodeRenderer, each: &mut dyn FnMut(&NodeRenderer) -> NodeRenderer) -> NodeRenderer {
    let mut one = |p: &NodeRenderer| Box::new(each(p));
    match r {
        NodeRenderer::Add(set) => NodeRenderer::Add(set.iter().map(&mut *each).collect()),
        NodeRenderer::Mul(set) => NodeRenderer::Mul(set.iter().map(&mut *each).collect()),
        NodeRenderer::Join(set) => NodeRenderer::Join(set.iter().map(&mut *each).collect()),
        NodeRenderer::Sub(a, b) => NodeRenderer::Sub(one(a), one(b)),
        NodeRenderer::Div(a, b) => NodeRenderer::Div(one(a), one(b)),
        NodeRenderer::Pow(a, b) => NodeRenderer::Pow(one(a), one(b)),
        NodeRenderer::Zip(f, a, b) => NodeRenderer::Zip(*f, one(a), one(b)),
        NodeRenderer::Map(f, x) => NodeRenderer::Map(*f, one(x)),
        NodeRenderer::Channel { x, k } => NodeRenderer::Channel { x: one(x), k: *k },
        NodeRenderer::Crop {
            x,
            window,
            a,
            b,
            rise,
            fall,
        } => NodeRenderer::Crop {
            x: one(x),
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
            x: one(x),
            cutoff: one(cutoff),
            q: one(q),
            gain: one(gain),
        },
        NodeRenderer::Physics { site, from, args } => NodeRenderer::Physics {
            site: *site,
            from: *from,
            args: args.iter().map(&mut *each).collect(),
        },
        NodeRenderer::Formula {
            formula,
            width,
            time,
        } => NodeRenderer::Formula {
            formula: formula.clone(),
            width: *width,
            time: one(time),
        },
        NodeRenderer::Indexed { slot, index, reach } => NodeRenderer::Indexed {
            slot: *slot,
            index: held(index, &mut one),
            reach: *reach,
        },
        NodeRenderer::Instant(index) => NodeRenderer::Instant(held(index, &mut one)),
        leaf => leaf.clone(),
    }
}

fn held(index: &Index, one: &mut dyn FnMut(&NodeRenderer) -> Box<NodeRenderer>) -> Index {
    let mapped = index.mapped(&mut |t| Ok::<_, std::convert::Infallible>(one(t)));
    match mapped {
        Ok(index) => index,
    }
}
