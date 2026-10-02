// Concern: lowers one node into the per-sample program that writes its value, each other value it reads a slot | Non-concern: holding or evaluating values (mod.rs) | IO: (NodeId) -> Program

use std::collections::HashMap;

use sva_formula::{Body, ClosedForm, Fold, NodeId, Part, Reads, Var};
use sva_samples::{
    Audible, Binary, BufId, CollapseError, Formula, Grid, Index, Map, NodeRenderer, Site, SiteId,
    Slot, Unary, Wrap, Written, truncate_spectral_sum, truncate_written_with,
};

use super::support::{Supports, landed, placed, reach, shifted, window};
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::refs::nodes_in;
use crate::time::{Affine, Lattice, Q};
use crate::typing::{Step, Typing, Value, When};
use sva_formula::series::written_out;

/// What a slot reads: a node's own value, or a closed form of `t` a node wrote inside its own
/// body, which is a value of its own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    Node(NodeId),
    Formula(ClosedForm),
}

/// One node's program, the value behind each slot, and the width each slot is read at.
pub(crate) struct Program {
    pub(crate) renderer: NodeRenderer,
    pub(crate) reads: Vec<Source>,
    pub(crate) sites: Vec<Site>,
    /// The most seconds a read was moved to land on a whole sample.
    pub(crate) moved: f64,
}

/// `id`'s program on its own grid: its operations, filters, solvers and reads inline, every
/// other value it reads a slot. A closed form that reads other nodes reads each at the whole
/// sample its shift rounds to.
pub(crate) fn of(
    tys: &Typing,
    supports: &Supports,
    (id, grid): (NodeId, Grid),
    (profile, bounds): (&sva_samples::Profile, &std::collections::BTreeSet<NodeId>),
) -> Result<Program, EngineError> {
    let mut build = Build {
        tys,
        supports,
        profile,
        bounds,
        owner: id,
        fine: grid != tys.grid(id),
        grid,
        reads: Vec::new(),
        sites: Vec::new(),
        moved: Q::ZERO,
    };
    let renderer = build.owner()?;
    Ok(Program {
        renderer,
        reads: build.reads,
        sites: build.sites,
        moved: build.moved.to_f64(),
    })
}

struct Build<'a> {
    tys: &'a Typing,
    supports: &'a Supports<'a>,
    profile: &'a sva_samples::Profile,
    /// Nodes read as values of their own, never inlined into a reader's program.
    bounds: &'a std::collections::BTreeSet<NodeId>,
    owner: NodeId,
    /// Evaluated finer than its own typing's step, as an alias score's reference is: only a
    /// closed form is.
    fine: bool,
    grid: Grid,
    reads: Vec<Source>,
    sites: Vec<Site>,
    moved: Q,
}

impl Build<'_> {
    fn owner(&mut self) -> Result<NodeRenderer, EngineError> {
        match self.tys.value(self.owner) {
            Value::ClosedForm(form) if form.var == Var::T => self.body(&form.body.clone()),
            _ => self.of(self.owner),
        }
    }

    fn of(&mut self, id: NodeId) -> Result<NodeRenderer, EngineError> {
        if id != self.owner && self.bounds.contains(&id) {
            return Ok(self.slot(Source::Node(id), Map::shift(0)));
        }
        match self.tys.value(id).clone() {
            Value::ClosedForm(form) => match (
                &form.body,
                crate::lower::constant_value(&form.body, form.var),
            ) {
                (Body::Const(c), _) if c.im == 0.0 => Ok(NodeRenderer::Const(c.re)),
                (_, Some(v)) => Ok(NodeRenderer::Const(v)),
                _ => Ok(self.slot(Source::Node(id), Map::shift(0))),
            },
            Value::Cast(Cast::Sample, source) => Ok(self.slot(Source::Node(source), Map::shift(0))),
            Value::Cast(Cast::Fourier | Cast::IFourier, _) if id != self.owner => {
                Ok(self.slot(Source::Node(id), Map::shift(0)))
            }
            Value::Cast(..) => Err(uncollapsed(self.tys, id)),
            Value::Read { source, at, .. } => self.read(source, at),
            Value::SelfAt { at } => self.own(at),
            Value::Noise(seed) => Ok(NodeRenderer::Noise(seed)),
            Value::Stored(_) => Ok(self.slot(Source::Node(id), Map::shift(0))),
            Value::Solver { .. } if id != self.owner => {
                Ok(self.slot(Source::Node(id), Map::shift(0)))
            }
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

    /// A closed form's own body: each node it reads a slot at the whole sample its shift
    /// rounds to, each subterm reading none a value of its own.
    fn body(&mut self, body: &Body) -> Result<NodeRenderer, EngineError> {
        if nodes_in(body).is_empty() {
            return Ok(self.formula(body.clone()));
        }
        let each = |build: &mut Self, parts: &[Part]| -> Result<Vec<NodeRenderer>, EngineError> {
            parts.iter().map(|p| build.body(&p.body)).collect()
        };
        Ok(match body {
            Body::Node(id) => self.slot(Source::Node(*id), Map::shift(0)),
            Body::Shift { by, of } if matches!(&*of.body, Body::Node(_)) => {
                let Body::Node(id) = &*of.body else {
                    unreachable!("the guard's own node");
                };
                let map = placed(*by, self.grid).ok_or_else(|| unplaced(self.tys, *id))?;
                self.moving(shifted(*by));
                self.slot(Source::Node(*id), map)
            }
            Body::Warp { at, of } if matches!(&*of.body, Body::Node(_)) => {
                let Body::Node(id) = &*of.body else {
                    unreachable!("the guard's own node");
                };
                let formula = warped(self.tys, *id, self.grid, self.profile)?
                    .ok_or_else(|| unevaluated(self.tys, self.owner, "a warped time"))?;
                NodeRenderer::Formula {
                    formula,
                    width: usize::from(self.tys.ty(*id).width),
                    time: Box::new(self.time_body(&at.body)?),
                }
            }
            Body::Add(parts) => NodeRenderer::Add(self.grouped(
                &flat(parts, |b| match b {
                    Body::Add(inner) => Some(inner),
                    _ => None,
                }),
                Body::Add,
            )?),
            Body::Mul(parts) => NodeRenderer::Mul(self.grouped(
                &flat(parts, |b| match b {
                    Body::Mul(inner) => Some(inner),
                    _ => None,
                }),
                Body::Mul,
            )?),
            Body::Div(a, b) => {
                NodeRenderer::Div(Box::new(self.body(&a.body)?), Box::new(self.body(&b.body)?))
            }
            Body::Pow(base, n) => NodeRenderer::Pow(
                Box::new(self.body(&base.body)?),
                Box::new(NodeRenderer::Const(f64::from(*n))),
            ),
            Body::Apply(op, x) => NodeRenderer::Map((*op).into(), Box::new(self.body(&x.body)?)),
            Body::Fold(op, parts) => {
                let [a, b] = parts.as_slice() else {
                    return Err(unevaluated(self.tys, self.owner, "a fold"));
                };
                NodeRenderer::Zip(
                    binary(*op),
                    Box::new(self.body(&a.body)?),
                    Box::new(self.body(&b.body)?),
                )
            }
            Body::Join(parts) => NodeRenderer::Join(each(self, parts)?),
            Body::Channel(x, k) => NodeRenderer::Channel {
                x: Box::new(self.body(&x.body)?),
                k: usize::from(*k),
            },
            Body::Crop {
                of,
                l,
                r,
                rise,
                fall,
            } => {
                let held = window(self.grid, l.value(), r.value());
                NodeRenderer::Crop {
                    x: Box::new(self.body(&of.body)?),
                    window: (held.start, held.end),
                    a: l.value(),
                    b: r.value(),
                    rise: *rise,
                    fall: *fall,
                }
            }
            // A finite sum is its terms; an infinite one over an opaque node never ends.
            Body::Series(_) => match (inlined(self.tys, body), written_out(body)) {
                (Some(free), _) => self.formula(free),
                (None, Some(written)) => self.body(&written)?,
                (None, None) => return Err(unsummed(self.tys, self.owner)),
            },
            other => match inlined(self.tys, other) {
                Some(free) => self.formula(free),
                None => return Err(unevaluated(self.tys, self.owner, "a construct")),
            },
        })
    }

    /// The parts reading no node gathered into one value, the rest each lowered.
    fn grouped(
        &mut self,
        parts: &[Part],
        whole: fn(Vec<Part>) -> Body,
    ) -> Result<Vec<NodeRenderer>, EngineError> {
        let (free, reading): (Vec<&Part>, Vec<&Part>) =
            parts.iter().partition(|p| nodes_in(&p.body).is_empty());
        let mut out = Vec::with_capacity(reading.len() + 1);
        match free.as_slice() {
            [] => {}
            [one] => out.push(self.formula((*one.body).clone())),
            many => out.push(self.formula(whole(many.iter().map(|p| (*p).clone()).collect()))),
        }
        for part in reading {
            out.push(self.body(&part.body)?);
        }
        Ok(out)
    }

    /// A subterm reading no node: its number, or the value its closed form is.
    fn formula(&mut self, body: Body) -> NodeRenderer {
        match crate::lower::constant_value(&body, Var::T) {
            Some(v) => NodeRenderer::Const(v),
            None => {
                let form = ClosedForm {
                    var: Var::T,
                    body,
                    origin: sva_formula::Origin::UNKNOWN,
                };
                self.slot(Source::Formula(form), Map::shift(0))
            }
        }
    }

    /// Notes how far landing `time` on a whole sample moved it.
    fn moving(&mut self, time: Option<Affine>) {
        if let Some(off) = time.and_then(|time| self.grid.moved(time)) {
            self.moved = match off.sub(self.moved).is_some_and(|d| d.num() > 0) {
                true => off,
                false => self.moved,
            };
        }
    }

    /// A read at a whole sample, at a time that moves, or at an index.
    fn read(&mut self, source: NodeId, at: When) -> Result<NodeRenderer, EngineError> {
        if self.fine {
            return Err(unplaced(self.tys, self.owner));
        }
        if let Some(map) = landed(&at, self.grid) {
            if let When::At(time) = at {
                self.moving(Some(time));
            }
            return Ok(self.slot(Source::Node(source), map));
        }
        let time = match at {
            When::Moving(time) => self.time(time)?,
            When::At(_) | When::Index(_) => return Err(unplaced(self.tys, self.owner)),
            When::Step(step) if crate::schedule::anywhere(self.tys, source) => {
                NodeRenderer::Instant(self.index(&step)?)
            }
            When::Step(step) => {
                let slot = self.reads.len();
                self.reads.push(Source::Node(source));
                return self.indexed(Slot::Read(BufId(slot as u32)), &step);
            }
        };
        match warped(self.tys, source, self.grid, self.profile)? {
            Some(formula) => Ok(NodeRenderer::Formula {
                formula,
                width: usize::from(self.tys.ty(source).width),
                time: Box::new(time),
            }),
            None => Err(unplaced(self.tys, self.owner)),
        }
    }

    /// A loop's own past, by index: a whole sample before the one being written.
    fn own(&mut self, at: When) -> Result<NodeRenderer, EngineError> {
        if self.fine {
            return Err(unplaced(self.tys, self.owner));
        }
        match (landed(&at, self.grid), at) {
            (Some(map), _) => Ok(NodeRenderer::Read {
                slot: Slot::Own,
                map,
            }),
            (None, When::Step(step)) => self.indexed(Slot::Own, &step),
            (None, _) => Err(unplaced(self.tys, self.owner)),
        }
    }

    /// The stored sample at an integer each sample evaluates, and how far from the sample
    /// being written it lands where that is cheap to bound.
    fn indexed(&mut self, slot: Slot, step: &Step) -> Result<NodeRenderer, EngineError> {
        Ok(NodeRenderer::Indexed {
            slot,
            index: self.index(step)?,
            reach: reach(self.tys, step, self.grid),
        })
    }

    /// An exact index is its map on this program's grid; a time that moves is evaluated as
    /// any operand is.
    fn index(&mut self, step: &Step) -> Result<Index, EngineError> {
        Ok(match step {
            Step::Index(index) => Index::At(
                index
                    .map(self.grid)
                    .ok_or_else(|| unplaced(self.tys, self.owner))?,
            ),
            Step::Nearest(time, round) => Index::Step(Box::new(self.time(*time)?), *round),
            Step::Add(parts) => Index::Add(self.indices(parts)?),
            Step::Mul(parts) => Index::Mul(self.indices(parts)?),
            Step::Neg(part) => Index::Neg(Box::new(self.index(part)?)),
        })
    }

    fn indices(&mut self, parts: &[Step]) -> Result<Vec<Index>, EngineError> {
        parts.iter().map(|p| self.index(p)).collect()
    }

    /// A time a read moves to, as the machine computes it at each instant: a closed form in
    /// `t` exactly as written, each node it reads a slot; any other node as its program.
    fn time(&mut self, id: NodeId) -> Result<NodeRenderer, EngineError> {
        match self.tys.value(id) {
            Value::ClosedForm(form) if form.var == Var::T => self.time_body(&form.body.clone()),
            _ => self.of(id),
        }
    }

    fn time_body(&mut self, body: &Body) -> Result<NodeRenderer, EngineError> {
        if let Some(v) = crate::lower::constant_value(body, Var::T) {
            return Ok(NodeRenderer::Const(v));
        }
        if let Some(wrap) = wrapped(body) {
            return Ok(NodeRenderer::Wrap(wrap));
        }
        let each = |build: &mut Self, parts: &[Part]| -> Result<Vec<NodeRenderer>, EngineError> {
            parts.iter().map(|p| build.time_body(&p.body)).collect()
        };
        let one = |build: &mut Self, p: &Part| build.time_body(&p.body).map(Box::new);
        Ok(match body {
            Body::Line => NodeRenderer::Time,
            Body::Node(id) => self.slot(Source::Node(*id), Map::shift(0)),
            Body::Add(parts) => NodeRenderer::Add(each(self, parts)?),
            Body::Mul(parts) => NodeRenderer::Mul(each(self, parts)?),
            Body::Div(a, b) => NodeRenderer::Div(one(self, a)?, one(self, b)?),
            Body::Pow(base, n) => NodeRenderer::Pow(
                one(self, base)?,
                Box::new(NodeRenderer::Const(f64::from(*n))),
            ),
            Body::Apply(op, x) => NodeRenderer::Map((*op).into(), one(self, x)?),
            Body::Fold(op, parts) => {
                let [a, b] = parts.as_slice() else {
                    return Err(unevaluated(self.tys, self.owner, "a fold"));
                };
                NodeRenderer::Zip(binary(*op), one(self, a)?, one(self, b)?)
            }
            Body::Join(parts) => NodeRenderer::Join(each(self, parts)?),
            Body::Channel(x, k) => NodeRenderer::Channel {
                x: one(self, x)?,
                k: usize::from(*k),
            },
            Body::Series(_) => match written_out(body) {
                Some(written) => self.time_body(&written)?,
                None => self.body(body)?,
            },
            other => self.body(other)?,
        })
    }

    /// A slot reading `source` through `map`.
    fn slot(&mut self, source: Source, map: Map) -> NodeRenderer {
        let at = match self.reads.iter().position(|held| *held == source) {
            Some(at) => at,
            None => {
                self.reads.push(source);
                self.reads.len() - 1
            }
        };
        NodeRenderer::Read {
            slot: Slot::Read(BufId(at as u32)),
            map,
        }
    }

    /// Where call site `id`'s state starts: its own support's start, never where a run
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
        let args = match name {
            "+" => addends(self.tys, args, self.bounds),
            _ => args.to_vec(),
        };
        let mut lowered = Vec::with_capacity(args.len());
        for arg in &args {
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
                let held = window(self.grid, a, b);
                NodeRenderer::Crop {
                    x: Box::new(lowered.remove(0)),
                    window: (held.start, held.end),
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
                None => return Err(unplanned(self.tys, id, name)),
            },
        })
    }
}

/// A construct no sample evaluates part by part, every node it reads a closed form reading
/// none, read at the construct's own instant: the construct with each written in, as-if.
fn inlined(tys: &Typing, body: &Body) -> Option<Body> {
    match body {
        Body::Node(id) => match tys.value(*id) {
            Value::ClosedForm(form) if form.var == Var::T && nodes_in(&form.body).is_empty() => {
                Some(form.body.clone())
            }
            _ => None,
        },
        Body::Shift { of, .. } | Body::Warp { of, .. } | Body::Deriv { of, .. }
            if !nodes_in(&of.body).is_empty() =>
        {
            None
        }
        other => {
            let mut held = true;
            let out =
                sva_formula::closed_form::map_children(other, |p| match inlined(tys, &p.body) {
                    Some(body) => Part::new(p.origin, body),
                    None => {
                        held = false;
                        p.clone()
                    }
                });
            held.then_some(out)
        }
    }
}

/// `a + b + c` nests to the left; as one sum, the same fold from `+0` in the same order, a
/// span prunes a dead addend outright rather than leaving the sum around it.
fn addends(
    tys: &Typing,
    args: &[NodeId],
    bounds: &std::collections::BTreeSet<NodeId>,
) -> Vec<NodeId> {
    let (mut head, mut tails) = (args, Vec::new());
    while let Value::Op { name, args: inner } = tys.value(head[0])
        && name == "+"
        && !bounds.contains(&head[0])
    {
        tails.push(&head[1..]);
        head = inner;
    }
    let mut out = head.to_vec();
    for tail in tails.iter().rev() {
        out.extend_from_slice(tail);
    }
    out
}

fn binary(op: Fold) -> Binary {
    match op {
        Fold::Max => Binary::Max,
        Fold::Min => Binary::Min,
        Fold::Mod => Binary::Mod,
    }
}

/// What a read of `source` evaluates at an instant between its samples: the noise, or its
/// closed form in `t` truncated to the band once, every ref it holds composed in exactly:
/// a warp reads a formula at its own instant. `None` where it holds state.
fn warped(
    tys: &Typing,
    source: NodeId,
    grid: Grid,
    profile: &sva_samples::Profile,
) -> Result<Option<Formula>, EngineError> {
    if !crate::schedule::anywhere(tys, source) {
        return Ok(None);
    }
    let form = match tys.value(source) {
        Value::Noise(seed) => {
            return Ok(Some(Formula::Drawn {
                seed: *seed,
                rate: grid.rate,
            }));
        }
        Value::Cast(Cast::Sample, of) => *of,
        _ => source,
    };
    let band = Audible::on(profile, grid);
    let truncated = |e: &sva_samples::CollapseError| collapse_refused(tys, form, e);
    let summed = crate::refs::spectral_sum_of(tys, form, Var::T)
        .ok()
        .map(|sum| truncate_spectral_sum(&sum, band));
    let refused = match summed {
        Some(Ok(sum)) => return Ok(Some(Formula::Sum(Box::new(sum)))),
        Some(Err(e)) => Some(truncated(&e)),
        None => None,
    };
    match tys.value(form) {
        Value::ClosedForm(written)
            if written.var == Var::T && crate::refs::reads_through(tys, &written.body, Var::T) =>
        {
            let shared = crate::refs::read_through(tys, |through| {
                let mut sharing = Sharing {
                    tys,
                    through,
                    named: HashMap::new(),
                    refs: Vec::new(),
                };
                let body = truncate_written_with(&written.body, band, through, &mut |id, band| {
                    sharing.name(id, band)
                })?;
                Ok(Written {
                    body,
                    refs: sharing.refs,
                })
            });
            Ok(Some(Formula::Written(Box::new(
                shared.map_err(|e| truncated(&e))?,
            ))))
        }
        _ => refused.map_or(Ok(None), Err),
    }
}

/// Each node a written form reads, truncated once per band it is read in, after what it reads.
struct Sharing<'a> {
    tys: &'a Typing,
    through: &'a dyn Reads,
    named: HashMap<(NodeId, [u64; 3]), NodeId>,
    refs: Vec<Body>,
}

impl Sharing<'_> {
    fn name(&mut self, id: NodeId, band: Audible) -> Result<NodeId, CollapseError> {
        if let Some(held) = self.named.get(&(id, band.key())) {
            return Ok(*held);
        }
        let Value::ClosedForm(form) = self.tys.value(id) else {
            unreachable!("a ref read through names a closed form");
        };
        let through = self.through;
        let body = truncate_written_with(&form.body, band, through, &mut |n, b| self.name(n, b))?;
        let named = NodeId(self.refs.len() as u32);
        self.refs.push(body);
        self.named.insert((id, band.key()), named);
        Ok(named)
    }
}

/// A line in `t` plus a multiple of one line modulo a positive number, each exact as
/// `loops::time_of` reads a time; `None` where no modulo is in it, or it is no such sum.
fn wrapped(f: &Body) -> Option<Wrap> {
    let Exact {
        line,
        wrap: Some((gain, inner, period)),
    } = exact(f)?
    else {
        return None;
    };
    let pair = |q: Q| (q.num(), q.den());
    Some(Wrap {
        scale: pair(line.scale),
        shift: pair(line.shift),
        gain: pair(gain),
        inner: [pair(inner.scale), pair(inner.shift)],
        period: pair(period),
    })
}

/// `line + gain*(inner mod period)`.
#[derive(Clone, Copy, PartialEq)]
struct Exact {
    line: Affine,
    wrap: Option<(Q, Affine, Q)>,
}

impl Exact {
    fn number(&self) -> Option<Q> {
        (self.line.scale.is_zero() && self.wrap.is_none()).then_some(self.line.shift)
    }

    fn scaled(self, k: Q) -> Option<Exact> {
        Some(Exact {
            line: Affine {
                scale: self.line.scale.mul(k)?,
                shift: self.line.shift.mul(k)?,
            },
            wrap: match self.wrap {
                Some((gain, inner, period)) => Some((gain.mul(k)?, inner, period)),
                None => None,
            },
        })
    }

    fn plus(self, o: Exact) -> Option<Exact> {
        let wrap = match (self.wrap, o.wrap) {
            (None, w) | (w, None) => w,
            (Some((g, inner, p)), Some((h, other, q))) if inner == other && p == q => {
                Some((g.add(h)?, inner, p))
            }
            _ => return None,
        };
        Some(Exact {
            line: Affine {
                scale: self.line.scale.add(o.line.scale)?,
                shift: self.line.shift.add(o.line.shift)?,
            },
            wrap,
        })
    }
}

fn exact(f: &Body) -> Option<Exact> {
    let number = |shift| Exact {
        line: Affine {
            scale: Q::ZERO,
            shift,
        },
        wrap: None,
    };
    match f {
        Body::Line => Some(Exact {
            line: Affine::NOW,
            wrap: None,
        }),
        Body::Const(c) if c.im == 0.0 => Some(number(Q::decimal(c.re)?)),
        Body::Add(parts) => parts
            .iter()
            .try_fold(number(Q::ZERO), |sum, p| sum.plus(exact(&p.body)?)),
        Body::Mul(parts) => {
            let (mut k, mut rest) = (Q::ONE, None);
            for p in parts {
                let factor = exact(&p.body)?;
                match (factor.number(), rest) {
                    (Some(n), _) => k = k.mul(n)?,
                    (None, None) => rest = Some(factor),
                    (None, Some(_)) => return None,
                }
            }
            rest.unwrap_or(number(Q::ONE)).scaled(k)
        }
        Body::Div(a, b) => exact(&a.body)?.scaled(Q::ONE.div(exact(&b.body)?.number()?)?),
        Body::Fold(Fold::Mod, parts) => {
            let [x, p] = parts.as_slice() else {
                return None;
            };
            let (x, p) = (exact(&x.body)?, exact(&p.body)?.number()?);
            (x.wrap.is_none() && !x.line.scale.is_zero() && p > Q::ZERO).then_some(Exact {
                line: Affine {
                    scale: Q::ZERO,
                    shift: Q::ZERO,
                },
                wrap: Some((Q::ONE, x.line, p)),
            })
        }
        _ => None,
    }
}

/// Every leaf under a renderer's operators.
pub(crate) fn leaves(renderer: &NodeRenderer, found: &mut dyn FnMut(&NodeRenderer)) {
    match renderer {
        NodeRenderer::Add(parts) | NodeRenderer::Mul(parts) | NodeRenderer::Join(parts) => {
            parts.iter().for_each(|p| leaves(p, found));
        }
        NodeRenderer::Sub(a, b)
        | NodeRenderer::Div(a, b)
        | NodeRenderer::Pow(a, b)
        | NodeRenderer::Zip(_, a, b) => {
            leaves(a, found);
            leaves(b, found);
        }
        NodeRenderer::Map(_, x)
        | NodeRenderer::Crop { x, .. }
        | NodeRenderer::Channel { x, .. } => leaves(x, found),
        NodeRenderer::Filter {
            x, cutoff, q, gain, ..
        } => [x, cutoff, q, gain]
            .into_iter()
            .for_each(|p| leaves(p, found)),
        NodeRenderer::Physics { args, .. } => args.iter().for_each(|p| leaves(p, found)),
        NodeRenderer::Formula { time, .. } => {
            leaves(time, found);
            found(renderer);
        }
        NodeRenderer::Indexed { index, .. } | NodeRenderer::Instant(index) => {
            index.times().into_iter().for_each(|t| leaves(t, found));
            found(renderer);
        }
        leaf => found(leaf),
    }
}

/// A time exactly placed past what a map's integers hold.
fn unplaced(tys: &Typing, owner: NodeId) -> EngineError {
    collapse_code(
        tys,
        owner,
        "a read's time lands past what exact integers place",
        "engine.unreadable_position",
    )
}

/// An infinite series over a node whose form no reader inlines: no sample of it ends.
fn unsummed(tys: &Typing, owner: NodeId) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "read.no_spectral_sum".to_string(),
        message: format!(
            "`{}` reads a node under an infinite series, and that node has no closed form to \
             sum it by",
            tys.name(owner)
        ),
        location: Located::at(tys.name(owner), None),
        help: "give the sum a finite upper bound, or write the series inside the node it reads"
            .to_string(),
    })
}

/// A node read under a construct no sample of the reader evaluates.
fn unevaluated(tys: &Typing, owner: NodeId, construct: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "read.no_spectral_sum".to_string(),
        message: format!(
            "`{}` reads a node under {construct}, which no sample of it evaluates",
            tys.name(owner)
        ),
        location: Located::at(tys.name(owner), None),
        help: "write the construct inside the node it reads, or read it off sample(...)"
            .to_string(),
    })
}

pub(crate) fn collapse_refused(
    tys: &Typing,
    id: NodeId,
    e: &sva_samples::CollapseError,
) -> EngineError {
    EngineError::refused(Diagnostic {
        code: e.code().to_string(),
        message: e.to_string(),
        location: Located::at(tys.name(id), None),
        help: e.help().to_string(),
    })
}

fn collapse_code(tys: &Typing, id: NodeId, message: &str, code: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message: message.to_string(),
        location: Located::at(tys.name(id), None),
        help: "write the node so the grid can hold it".to_string(),
    })
}

fn uncollapsed(tys: &Typing, id: NodeId) -> EngineError {
    collapse_code(
        tys,
        id,
        "a closed form reaches the grid with no collapse written for it",
        "type.samples_in_closed_form",
    )
}

fn unplanned(tys: &Typing, id: NodeId, name: &str) -> EngineError {
    collapse_code(
        tys,
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

/// A sum of sums, or a product of products, as one list of operands in written order, so a
/// long chain lowers without recursing once per operand.
fn flat(parts: &[Part], nested: fn(&Body) -> Option<&Vec<Part>>) -> Vec<Part> {
    let (mut out, mut stack) = (Vec::new(), vec![parts.iter()]);
    while let Some(top) = stack.last_mut() {
        match top.next() {
            None => {
                stack.pop();
            }
            Some(part) => match nested(&part.body) {
                Some(inner) => stack.push(inner.iter()),
                None => out.push(part.clone()),
            },
        }
    }
    out
}
