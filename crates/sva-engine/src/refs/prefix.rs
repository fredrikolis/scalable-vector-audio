// Concern: where a node's switches turn on the grid, and its identity with each switch not yet reached undone | Non-concern: storing runs under it | IO: (NodeId, index) -> instants, Hash

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::closed_form::map_children;
use sva_formula::{Body, C64, ClosedForm, Edge, Hash, NodeId, Part, normalize_closed_form};

use super::identity::{Sink, closed_form_identity, identity_in};
use super::substituted_closed_form;
use crate::cast::Cast;
use crate::error::EngineError;
use crate::typing::{Typing, Value};

/// What a render has asked of its nodes' switches, kept for every later ask.
#[derive(Default)]
pub(crate) struct Prefixes {
    points: BTreeMap<NodeId, BTreeSet<i64>>,
    prefixed: BTreeMap<(NodeId, i64), Hash>,
}

/// One walk over one typing at one rate, a node's plain identity read from `named`.
pub(crate) struct Walk<'a> {
    pub(crate) typing: &'a Typing,
    pub(crate) rate: u32,
    pub(crate) held: &'a mut Prefixes,
    pub(crate) named: &'a mut BTreeMap<NodeId, Hash>,
    open: Vec<NodeId>,
}

impl<'a> Walk<'a> {
    pub(crate) fn new(
        typing: &'a Typing,
        rate: u32,
        held: &'a mut Prefixes,
        named: &'a mut BTreeMap<NodeId, Hash>,
    ) -> Walk<'a> {
        Walk {
            typing,
            rate,
            held,
            named,
            open: Vec::new(),
        }
    }

    /// Every grid index, on the node's own clock, a switch under it turns at: a crop's
    /// opening, its closing, and each read's own switches moved by its shift. A switch this
    /// cannot place on the grid is left out, which only shares less.
    pub(crate) fn change_points(&mut self, id: NodeId) -> BTreeSet<i64> {
        if let Some(held) = self.held.points.get(&id) {
            return held.clone();
        }
        self.held.points.insert(id, BTreeSet::new());
        let mut out = BTreeSet::new();
        match self.typing.value(id) {
            Value::ClosedForm(_) => {
                if let Some(form) = substituted_closed_form(self.typing, id) {
                    body_points(&form.body, self.rate, &mut out);
                }
            }
            Value::Cast(Cast::Sample, source) => out = self.change_points(*source),
            Value::Read { source, at, .. } => {
                if let Ok(shift) = at.steps_at(self.rate) {
                    let moved = self.change_points(*source);
                    out.extend(moved.into_iter().map(|c| c.saturating_sub(shift)));
                }
            }
            Value::Filter {
                x, cutoff, q, gain, ..
            } => {
                for operand in [*x, *cutoff, *q, *gain] {
                    out.extend(self.change_points(operand));
                }
            }
            Value::Op { name, args } => {
                for arg in args.clone() {
                    out.extend(self.change_points(arg));
                }
                if let Some(window) = self.sampled_window(name, args) {
                    out.extend(window.switches(self.rate));
                }
            }
            Value::Cast(..) | Value::SelfAt(_) | Value::Grid(_) | Value::Solver(_) => {}
        }
        self.held.points.insert(id, out.clone());
        out
    }

    /// `id` as it stands before grid index `at`: every switch at or past it is what it
    /// switches from, and a node with none there is its own identity.
    pub(crate) fn prefix_identity(&mut self, id: NodeId, at: i64) -> Result<Hash, EngineError> {
        if !self.change_points(id).iter().any(|c| *c >= at) {
            return identity_in(self.typing, id, &mut self.named);
        }
        if let Some(held) = self.held.prefixed.get(&(id, at)) {
            return Ok(*held);
        }
        if self.open.contains(&id) {
            return Err(super::cyclic(self.typing, id));
        }
        self.open.push(id);
        let found = self.prefixed_of(id, at);
        self.open.pop();
        let found = found?;
        self.held.prefixed.insert((id, at), found);
        Ok(found)
    }

    fn prefixed_of(&mut self, id: NodeId, at: i64) -> Result<Hash, EngineError> {
        let typing = self.typing;
        let mut sink = Sink::new();
        match typing.value(id) {
            Value::ClosedForm(_) => {
                let form = substituted_closed_form(typing, id)
                    .expect("a closed form with switches was substituted");
                let body = before(&form.body, at, self.rate);
                return Ok(form_identity(ClosedForm { body, ..form }));
            }
            Value::Cast(cast, source) => {
                sink.text(cast.name());
                sink.hash(self.prefix_identity(*source, at)?);
            }
            Value::Read {
                source, at: offset, ..
            } => {
                let shift = offset
                    .steps_at(self.rate)
                    .expect("a read with switches is on the grid");
                sink.text("read");
                sink.hash(self.prefix_identity(*source, at.saturating_add(shift))?);
                super::identity::offset(&mut sink, *offset);
            }
            Value::Filter {
                shape,
                x,
                cutoff,
                q,
                gain,
            } => {
                sink.text(shape.name());
                for operand in [*x, *cutoff, *q, *gain] {
                    sink.hash(self.prefix_identity(operand, at)?);
                }
            }
            Value::Op { name, args } => {
                let window = self.sampled_window(name, args);
                let var = typing.var(id);
                match window.map(|w| w.before(at, self.rate)) {
                    Some(Before::Zero) => return Ok(constant_identity(0.0, var)),
                    Some(Before::Open(window)) => {
                        sink.text(name);
                        sink.hash(self.prefix_identity(args[0], at)?);
                        for edge in window.args() {
                            sink.hash(constant_identity(edge, var));
                        }
                    }
                    Some(Before::Same) | None => {
                        sink.text(name);
                        for arg in args.clone() {
                            sink.hash(self.prefix_identity(arg, at)?);
                        }
                    }
                }
            }
            Value::SelfAt(_) | Value::Grid(_) | Value::Solver(_) => {
                return identity_in(typing, id, &mut self.named);
            }
        }
        Ok(sink.finish())
    }

    /// A sampled crop's window, its edges and shoulders each a number.
    fn sampled_window(&self, name: &str, args: &[NodeId]) -> Option<Window> {
        if name != "crop" {
            return None;
        }
        let number = |at: usize| match args.get(at) {
            None => Some(0.0),
            Some(id) => match self.typing.value(*id) {
                Value::ClosedForm(form) => match form.body {
                    Body::Const(c) if c.im == 0.0 => Some(c.re),
                    _ => crate::lower::constant_value(&form.body, form.var),
                },
                _ => None,
            },
        };
        Some(Window {
            l: number(1)?,
            r: number(2)?,
            rise: number(3)?,
            fall: number(4)?,
        })
    }
}

/// A crop's `[l, r)` and shoulders, as its evaluators read them.
#[derive(Clone, Copy)]
struct Window {
    l: f64,
    r: f64,
    rise: f64,
    fall: f64,
}

enum Before {
    /// Nothing opened yet: zero.
    Zero,
    /// Open, and nothing closed yet: the same window with no end.
    Open(Window),
    Same,
}

impl Window {
    fn switches(self, rate: u32) -> impl Iterator<Item = i64> {
        [opens(rate, self.l), closes(rate, self.r, self.fall)]
            .into_iter()
            .flatten()
    }

    fn before(self, at: i64, rate: u32) -> Before {
        if opens(rate, self.l).is_some_and(|c| c >= at) {
            return Before::Zero;
        }
        match closes(rate, self.r, self.fall) {
            Some(c) if c >= at => Before::Open(Window {
                r: f64::INFINITY,
                fall: 0.0,
                ..self
            }),
            _ => Before::Same,
        }
    }

    /// The edges and shoulders a sampled crop of this window is written with.
    fn args(self) -> Vec<f64> {
        match self.rise > 0.0 || self.fall > 0.0 {
            true => vec![self.l, self.r, self.rise, self.fall],
            false => vec![self.l, self.r],
        }
    }
}

/// The first index any evaluator reads at or past `l`: before it a crop is zero.
fn opens(rate: u32, l: f64) -> Option<i64> {
    l.is_finite()
        .then(|| crate::render::extent::window(rate, l, f64::INFINITY).start)
}

/// Before the first index any evaluator reads at or past `r - fall` a crop's gain is the
/// one it has with no end; a fall's own rounding moves that one sample earlier.
fn closes(rate: u32, r: f64, fall: f64) -> Option<i64> {
    let edge = r - fall;
    let first = edge
        .is_finite()
        .then(|| crate::render::extent::window(rate, edge, f64::INFINITY).start)?;
    Some(match fall > 0.0 {
        true => first - 1,
        false => first,
    })
}

fn body_points(f: &Body, rate: u32, out: &mut BTreeSet<i64>) {
    match f {
        // A time moved or warped reads its switches at instants this does not place.
        Body::Shift { .. } | Body::Warp { .. } => {}
        Body::Crop { of, l, r, fall, .. } => {
            out.extend(opens(rate, l.value()));
            out.extend(closes(rate, r.value(), *fall));
            body_points(&of.body, rate, out);
        }
        other => {
            for p in sva_formula::closed_form::children(other) {
                body_points(&p.body, rate, out);
            }
        }
    }
}

fn before(f: &Body, at: i64, rate: u32) -> Body {
    match f {
        Body::Shift { .. } | Body::Warp { .. } => f.clone(),
        Body::Crop {
            of,
            l,
            r,
            rise,
            fall,
        } => {
            if opens(rate, l.value()).is_some_and(|c| c >= at) {
                return Body::Const(C64::ZERO);
            }
            let inner = Part::new(of.origin, before(&of.body, at, rate));
            match closes(rate, r.value(), *fall) {
                Some(c) if c >= at => Body::Crop {
                    of: inner,
                    l: *l,
                    r: Edge::PosInf,
                    rise: *rise,
                    fall: 0.0,
                },
                _ => Body::Crop {
                    of: inner,
                    l: *l,
                    r: *r,
                    rise: *rise,
                    fall: *fall,
                },
            }
        }
        other => map_children(other, |p| Part::new(p.origin, before(&p.body, at, rate))),
    }
}

fn form_identity(form: ClosedForm) -> Hash {
    let sum = normalize_closed_form(&form);
    closed_form_identity(&sum, Some(&form)).expect("a written form always hashes")
}

fn constant_identity(v: f64, var: sva_formula::Var) -> Hash {
    form_identity(ClosedForm {
        var,
        body: Body::Const(C64::real(v)),
        origin: sva_formula::Origin::UNKNOWN,
    })
}

#[cfg(test)]
mod tests {
    use crate::render::{Render, RenderConfig, plan};

    const PAD: &str = "lowpass(sample(0.3*vel*(saw(f0*8ct) + saw(f0/8ct))*(crop(min(t/0.005, 1)\
        *(0.6 + 0.4*exp(-t/0.25)), 0s, release) + crop(min(release/0.005, 1)*(0.6 + 0.4\
        *exp(-release/0.25))*exp(-(t - release)/0.3), release, 3600s))), \
        cutoff=min(f0*(2 + 10*vel), 18000), q=0.9)\n";

    fn planned(root: &str) -> Render {
        let mut files = sva_ast::Composition::new();
        files
            .insert("pad", PAD)
            .insert("held", "@pad(t, f0=220, vel=0.6)\n")
            .insert("released", "@pad(t, f0=220, vel=0.6, release=0.61237)\n")
            .insert(
                "spectrum",
                "ifourier(fourier(crop(sin(2*pi*t), 0s, 0.5s)))\n",
            )
            .insert("env", "crop(sin(2*pi*3*t), 0.25s, 0.5s, fall=0.1s)\n")
            .insert("late", "@env(t - 100sp)\n")
            .insert("now", "sample(@env)\n");
        let g = sva_ast::load(&files).expect("a composition");
        plan(&g, root, RenderConfig::seconds(44_100, 1.0)).expect("a plan")
    }

    fn asked<T>(root: &str, ask: impl FnOnce(&mut super::Walk, sva_formula::NodeId) -> T) -> T {
        let held = planned(root);
        let id = held.id(root).expect("the root");
        held.prefixes(|walk| ask(walk, id))
    }

    /// Up to the index its release lands at, a released note is the held one.
    #[test]
    fn a_released_pad_is_the_held_pad_before_its_release() {
        let held = asked("held", |walk, id| walk.prefix_identity(id, i64::MAX));
        let at = (0.61237f64 * 44_100.0).ceil() as i64;
        asked("released", |walk, id| {
            assert!(walk.change_points(id).contains(&at));
            for (before, same) in [(1, true), (at, true), (at + 1, false)] {
                let prefix = walk.prefix_identity(id, before).expect("an identity");
                assert_eq!(
                    prefix == held.clone().expect("an identity"),
                    same,
                    "{before}"
                );
            }
        });
    }

    /// A transform reads every instant, so none of its switches is one of its own; a read
    /// moves its node's switches by its shift.
    #[test]
    fn a_transform_hides_its_switches_and_a_read_moves_them() {
        assert!(asked("spectrum", |walk, id| walk.change_points(id)).is_empty());
        let now = asked("now", |walk, id| walk.change_points(id));
        let late = asked("late", |walk, id| walk.change_points(id));
        assert_eq!(now, [11_025, 17_639].into());
        assert_eq!(late, now.iter().map(|c| c + 100).collect());
    }
}
