// Concern: where a node's switches turn on the grid, and its identity with each switch not yet reached undone | Non-concern: storing runs under it | IO: (NodeId, index) -> instants, Hash

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::closed_form::map_children;
use sva_formula::{Body, C64, ClosedForm, Edge, Hash, NodeId, Part, normalize_closed_form};
use sva_samples::Grid;

use super::identity::{Sink, closed_form_identity, gridded, identity_in};
use super::substituted_closed_form;
use crate::error::EngineError;
use crate::lower::field;
use crate::typing::{Typing, Value, When};

/// What a render has asked of its nodes' switches, kept for every later ask.
#[derive(Default)]
pub(crate) struct Prefixes {
    points: BTreeMap<NodeId, BTreeSet<i64>>,
    prefixed: BTreeMap<(NodeId, i64), Hash>,
}

/// One walk over one typing, a node's plain identity read from `named`.
pub(crate) struct Walk<'a> {
    pub(crate) typing: &'a Typing,
    pub(crate) held: &'a mut Prefixes,
    pub(crate) named: &'a mut BTreeMap<NodeId, Hash>,
    open: Vec<NodeId>,
}

impl<'a> Walk<'a> {
    pub(crate) fn new(
        typing: &'a Typing,
        held: &'a mut Prefixes,
        named: &'a mut BTreeMap<NodeId, Hash>,
    ) -> Walk<'a> {
        Walk {
            typing,
            held,
            named,
            open: Vec::new(),
        }
    }

    /// Where a machine's sampled crops open and close on its own clock, reads moved by their
    /// shift. Rows sum a closed form and its prefix form in different orders, so they hold none.
    pub(crate) fn change_points(&mut self, id: NodeId) -> BTreeSet<i64> {
        if let Some(held) = self.held.points.get(&id) {
            return held.clone();
        }
        self.held.points.insert(id, BTreeSet::new());
        let mut out = BTreeSet::new();
        if self.typing.sum_slots(id).is_some() {
            return out;
        }
        let grid = self.grid(id);
        match self.typing.value(id) {
            Value::Read { source, at, .. } => {
                // A scaled or moving read switches nowhere: its prefix is its whole identity.
                if let Some(lead) = self.lead(id, *source, at) {
                    let moved = self.change_points(*source);
                    out.extend(moved.into_iter().map(|c| c.saturating_sub(lead)));
                }
            }
            Value::Filter {
                x, cutoff, q, gain, ..
            } => {
                for operand in [*x, *cutoff, *q, *gain] {
                    out.extend(self.argument_points(operand));
                }
            }
            Value::Solver { varying, .. } => {
                for (_, arg) in varying.clone() {
                    out.extend(self.argument_points(arg));
                }
            }
            Value::Op { name, args } => {
                for arg in args.clone() {
                    out.extend(self.change_points(arg));
                }
                if let Some(window) = self.sampled_window(name, args) {
                    out.extend(window.switches(grid));
                }
            }
            Value::ClosedForm(_) | Value::Cast(..) | Value::SelfAt { .. } | Value::Noise(_) => {}
        }
        self.held.points.insert(id, out.clone());
        out
    }

    /// A closed form a machine reads as an argument is evaluated there instant by instant, so
    /// its crops switch where the machine's own would.
    fn argument_points(&mut self, arg: NodeId) -> BTreeSet<i64> {
        let mut out = BTreeSet::new();
        match self.pointwise(arg) {
            Some(form) => body_points(&form.body, self.grid(arg), &mut out),
            None => out = self.change_points(arg),
        }
        out
    }

    fn pointwise(&self, arg: NodeId) -> Option<ClosedForm> {
        crate::lower::inline::renderer(self.typing, arg)?;
        substituted_closed_form(self.typing, arg)
    }

    /// An argument before `at`: the number its prefix form folds to, or that form's identity.
    fn argument_before(&mut self, arg: NodeId, at: i64) -> Result<Argued, EngineError> {
        let Some(form) = self.pointwise(arg) else {
            return self.prefix_identity(arg, at).map(Argued::Node);
        };
        let (mut points, grid) = (BTreeSet::new(), self.grid(arg));
        body_points(&form.body, grid, &mut points);
        if !points.iter().any(|c| *c >= at) {
            return identity_in(self.typing, arg, self.named).map(Argued::Node);
        }
        let body = before(&form.body, at, grid);
        Ok(match crate::lower::constant_value(&body, form.var) {
            Some(v) => Argued::Number(v + 0.0),
            None => Argued::Node(form_identity(ClosedForm { body, ..form })),
        })
    }

    /// `id` as it stands before grid index `at`: every switch at or past it is what it
    /// switches from, and a node with none there is its own identity.
    pub(crate) fn prefix_identity(&mut self, id: NodeId, at: i64) -> Result<Hash, EngineError> {
        if !self.change_points(id).iter().any(|c| *c >= at) {
            return identity_in(self.typing, id, self.named);
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
            Value::Read {
                source, at: when, ..
            } => {
                let lead = self
                    .lead(id, *source, when)
                    .expect("a read with switches is at scale one");
                sink.text("read");
                sink.hash(self.prefix_identity(*source, at.saturating_add(lead))?);
                super::identity::when(&mut sink, typing, *when);
            }
            Value::Filter {
                shape,
                x,
                cutoff,
                q,
                gain,
            } => {
                sink.text(shape.name());
                sink.hash(self.prefix_identity(*x, at)?);
                for operand in [*cutoff, *q, *gain] {
                    sink.hash(match self.argument_before(operand, at)? {
                        Argued::Number(v) => constant_identity(v, typing.var(operand)),
                        Argued::Node(held) => held,
                    });
                }
            }
            Value::Solver { params, varying } => {
                let mut held = (**params).clone();
                let mut read = Vec::new();
                for (key, arg) in varying {
                    match self.argument_before(*arg, at)? {
                        Argued::Number(v) => *field(&mut held, key).expect("a varying field") = v,
                        Argued::Node(h) => read.push((*key, h)),
                    }
                }
                for (key, _) in &read {
                    *field(&mut held, key).expect("a varying field") = f64::NAN;
                }
                sink.text(&format!("{held:?}"));
                for (key, h) in read {
                    sink.text(key);
                    sink.hash(h);
                }
            }
            Value::Op { name, args } => {
                let window = self.sampled_window(name, args);
                let var = typing.var(id);
                match window.map(|w| w.before(at, self.grid(id))) {
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
            Value::ClosedForm(_) | Value::Cast(..) | Value::SelfAt { .. } | Value::Noise(_) => {
                return identity_in(typing, id, self.named);
            }
        }
        Ok(gridded(typing, id, sink.finish()))
    }

    fn grid(&self, id: NodeId) -> sva_samples::Grid {
        self.typing.grid(id)
    }

    /// How far past its own sample a read at scale one reaches its source, where every sample
    /// reaches it alike.
    fn lead(&self, id: NodeId, source: NodeId, at: &When) -> Option<i64> {
        let map = at.map(self.typing.grid(id), self.typing.grid(source))?;
        (map.a == map.d && map.least() == map.lead()).then(|| map.lead())
    }

    /// A sampled crop's window, its edges and shoulders each a number.
    fn sampled_window(&self, name: &str, args: &[NodeId]) -> Option<Window> {
        if name != "crop" {
            return None;
        }
        let number = |at: usize| match args.get(at) {
            None => Some(0.0),
            Some(id) => crate::lower::number_of(self.typing, *id),
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

/// An argument before an instant: one number, or a form still moving.
enum Argued {
    Number(f64),
    Node(Hash),
}

enum Before {
    /// Nothing opened yet: zero.
    Zero,
    /// Open, and nothing closed yet: the same window with no end.
    Open(Window),
    Same,
}

impl Window {
    fn switches(self, grid: Grid) -> impl Iterator<Item = i64> {
        [opens(grid, self.l), closes(grid, self.r, self.fall)]
            .into_iter()
            .flatten()
    }

    fn before(self, at: i64, grid: Grid) -> Before {
        if opens(grid, self.l).is_some_and(|c| c >= at) {
            return Before::Zero;
        }
        match closes(grid, self.r, self.fall) {
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
fn opens(grid: Grid, l: f64) -> Option<i64> {
    l.is_finite()
        .then(|| crate::render::extent::window(grid, l, f64::INFINITY).start)
}

/// Before the first index any evaluator reads at or past `r - fall` a crop's gain is the
/// one it has with no end; a fall's own rounding moves that one sample earlier.
fn closes(grid: Grid, r: f64, fall: f64) -> Option<i64> {
    let edge = r - fall;
    let first = edge
        .is_finite()
        .then(|| crate::render::extent::window(grid, edge, f64::INFINITY).start)?;
    Some(match fall > 0.0 {
        true => first - 1,
        false => first,
    })
}

fn body_points(f: &Body, grid: Grid, out: &mut BTreeSet<i64>) {
    match f {
        // A time moved or warped reads its switches at instants this does not place.
        Body::Shift { .. } | Body::Warp { .. } => {}
        Body::Crop { of, l, r, fall, .. } => {
            out.extend(opens(grid, l.value()));
            out.extend(closes(grid, r.value(), *fall));
            body_points(&of.body, grid, out);
        }
        other => {
            for p in sva_formula::closed_form::children(other) {
                body_points(&p.body, grid, out);
            }
        }
    }
}

fn before(f: &Body, at: i64, grid: Grid) -> Body {
    match f {
        Body::Shift { .. } | Body::Warp { .. } => f.clone(),
        Body::Crop {
            of,
            l,
            r,
            rise,
            fall,
        } => {
            if opens(grid, l.value()).is_some_and(|c| c >= at) {
                return Body::Const(C64::ZERO);
            }
            let inner = Part::new(of.origin, before(&of.body, at, grid));
            match closes(grid, r.value(), *fall) {
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
        other => map_children(other, |p| Part::new(p.origin, before(&p.body, at, grid))),
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

    const RATE: u32 = 8_000;

    const PAD: &str = "release = inf\nlowpass(sample(0.3*vel*saw(f0)*(crop(1, 0s, release) + crop(exp(-(t - \
        release)/0.3), release, 3600s))), cutoff=900, q=0.9)\n";

    fn planned(root: &str) -> Render {
        let mut files = sva_ast::Composition::new();
        files
            .insert(
                "note",
                "release = inf\ncrop(sample(sin(2*pi*f0*t)), 0s, release)\n",
            )
            .insert("held", "@note(t, f0=220)\n")
            .insert("released", "@note(t, f0=220, release=0.61237)\n")
            .insert("pad", PAD)
            .insert(
                "pad_released",
                "@pad(t, f0=220, vel=0.6, release=0.61237)\n",
            )
            .insert(
                "spectrum",
                "ifourier(fourier(crop(sin(2*pi*t), 0s, 0.5s)))\n",
            )
            .insert(
                "env",
                "crop(sample(sin(2*pi*3*t)), 0.25s, 0.5s, fall=0.1s)\n",
            )
            .insert("late", "@env(t - 100sp)\n");
        let g = sva_ast::load(&files).expect("a composition");
        plan(&g, root, RenderConfig::seconds(RATE, 1.0)).expect("a plan")
    }

    fn asked<T>(root: &str, ask: impl FnOnce(&mut super::Walk, sva_formula::NodeId) -> T) -> T {
        let held = planned(root);
        let id = held.id(root).expect("the root");
        held.prefixes(|walk| ask(walk, id))
    }

    /// Up to the index its release lands at, a released note is the held one.
    #[test]
    fn a_released_note_is_the_held_note_before_its_release() {
        let held =
            asked("held", |walk, id| walk.prefix_identity(id, i64::MAX)).expect("an identity");
        let at = (0.61237 * f64::from(RATE)).ceil() as i64;
        asked("released", |walk, id| {
            assert!(walk.change_points(id).contains(&at));
            for (before, same) in [(1, true), (at, true), (at + 1, false)] {
                let prefix = walk.prefix_identity(id, before).expect("an identity");
                assert_eq!(prefix == held, same, "{before}");
            }
        });
    }

    /// Neither a transform nor rows hold a switch; a read moves one by its shift.
    #[test]
    fn only_the_machine_places_a_switch_and_a_read_moves_it() {
        assert!(asked("spectrum", |walk, id| walk.change_points(id)).is_empty());
        assert!(asked("pad_released", |walk, id| walk.change_points(id)).is_empty());
        let now = asked("env", |walk, id| walk.change_points(id));
        let late = asked("late", |walk, id| walk.change_points(id));
        let rate = i64::from(RATE);
        assert_eq!(now, [rate / 4, rate * 2 / 5 - 1].into());
        assert_eq!(late, now.iter().map(|c| c + 100).collect());
    }
}
