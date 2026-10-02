// Concern: reads each ref of a written form as the form it names, each answer kept per node | Non-concern: which refs a caller lets it read (sva-engine) | IO: (NodeId) -> each reading of its form

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Deref;
use std::rc::Rc;

use crate::affine::{Axis, Coeff, Reading, key_moves_read, polynomial_read};
use crate::closed_form::{Body, IndexId, Var, map_children, read_at, read_at_with, shift_line};
use crate::env::NodeId;
use crate::hash::{Hash, hash_time};
use crate::origin::Origin;
use crate::refusal::{Factor, Left, LeftReason};
use crate::series::IndexGrowth;
use crate::spectral_sum::SpectralSum;
use crate::spectral_sum::build::{lower, timeless};
use crate::spectral_sum::image::left;
use crate::spectral_sum::kink;
pub use crate::spectral_sum::kink::{Crossing, Line};

/// What a ref in a written form answers each reading of it with, as the form it names would.
pub trait Reads {
    fn lowered(&self, id: NodeId, origin: Origin, var: Var) -> Result<SpectralSum, Left>;
    fn polynomial(&self, id: NodeId, reading: Reading) -> Option<Vec<Coeff>>;
    fn timeless(&self, id: NodeId) -> bool;
    fn key_moves(&self, id: NodeId) -> bool;
    fn mentions_line(&self, id: NodeId) -> bool;
    fn axis(&self, id: NodeId) -> Option<Axis>;
    fn line(&self, id: NodeId) -> Option<Line>;
    fn kink(&self, id: NodeId) -> Option<(Body, Crossing)>;
    /// A node standing for `id` with `kink` read as `with` throughout, where it reaches one.
    fn replaced(&self, id: NodeId, scope: usize, kink: &Body, with: &Body) -> Option<NodeId>;
    /// A node standing for `id` read at `at`, the time written through its form.
    fn moved(&self, id: NodeId, at: &Body) -> Option<NodeId>;
    fn stand_for(&self, body: Body) -> Option<NodeId>;
    fn scope(&self) -> usize;
    /// The form `id` names as `read` reads it, one level down: its own refs still refs.
    fn view(&self, id: NodeId, read: &Read) -> Option<Form<'_>>;
    /// How the form `id` names grows in `k`, read at a time `k` moves.
    fn growth(&self, id: NodeId, at: &Body, k: IndexId) -> Option<IndexGrowth>;
}

/// How a ref is read where it stands: as written, at a time, or shifted.
pub enum Read<'t> {
    As,
    At(&'t Body),
    By(f64),
}

/// A ref read as nothing but itself: a name whose form is unknown here.
pub struct Opaque;

impl Reads for Opaque {
    fn lowered(&self, _: NodeId, origin: Origin, _: Var) -> Result<SpectralSum, Left> {
        Err(left(origin, Factor::Value, LeftReason::Unsubstituted))
    }

    fn polynomial(&self, _: NodeId, _: Reading) -> Option<Vec<Coeff>> {
        None
    }

    fn timeless(&self, _: NodeId) -> bool {
        false
    }

    fn key_moves(&self, _: NodeId) -> bool {
        true
    }

    fn mentions_line(&self, _: NodeId) -> bool {
        false
    }

    fn axis(&self, _: NodeId) -> Option<Axis> {
        None
    }

    fn line(&self, _: NodeId) -> Option<Line> {
        None
    }

    fn kink(&self, _: NodeId) -> Option<(Body, Crossing)> {
        None
    }

    fn replaced(&self, _: NodeId, _: usize, _: &Body, _: &Body) -> Option<NodeId> {
        None
    }

    fn moved(&self, _: NodeId, _: &Body) -> Option<NodeId> {
        None
    }

    fn stand_for(&self, _: Body) -> Option<NodeId> {
        None
    }

    fn scope(&self) -> usize {
        0
    }

    fn view(&self, _: NodeId, _: &Read) -> Option<Form<'_>> {
        None
    }

    fn growth(&self, _: NodeId, _: &Body, _: IndexId) -> Option<IndexGrowth> {
        None
    }
}

/// Each reading of each node, kept by whoever holds the nodes for as long as they hold.
#[derive(Debug, Default)]
pub struct Kept {
    lowered: Each<(NodeId, Var), Result<SpectralSum, Left>>,
    polynomials: Each<(NodeId, Reading), Option<Vec<Coeff>>>,
    timeless: Each<NodeId, bool>,
    key_moves: Each<NodeId, bool>,
    mentions_line: Each<NodeId, bool>,
    axes: Each<NodeId, Axis>,
    lines: Each<NodeId, Option<Line>>,
    kinks: Each<NodeId, Option<(Body, Crossing)>>,
    views: Each<(NodeId, View), Rc<Body>>,
    growths: Each<(NodeId, IndexId, Hash), IndexGrowth>,
}

type Each<K, V> = RefCell<HashMap<K, V>>;

/// Which rewritten reading of a form a view is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum View {
    At(Hash),
    By(u64),
}

impl Kept {
    pub fn clear(&mut self) {
        *self = Kept::default();
    }
}

/// The written form each node names, `None` where the caller reads no form through it.
pub type Written<'a> = &'a dyn Fn(NodeId) -> Option<(&'a Body, Origin)>;

/// Reads refs through `written`, keeping each node's answers in `kept`. A form rewritten
/// below a ref stands in as a node of its own, numbered from the top of the ids down, kept
/// only while this reading lasts.
pub struct Through<'a> {
    written: Written<'a>,
    kept: &'a Kept,
    standing: Kept,
    stand_ins: RefCell<Vec<Rc<Body>>>,
    replacing: RefCell<HashMap<(NodeId, usize), Option<NodeId>>>,
    moving: Each<NodeId, Vec<(Body, Option<NodeId>)>>,
    scopes: Cell<usize>,
}

/// A form a reader hands out: one a node names, or one standing in for a rewritten form.
pub enum Form<'a> {
    Node(&'a Body),
    Stand(Rc<Body>),
}

impl Deref for Form<'_> {
    type Target = Body;

    fn deref(&self) -> &Body {
        match self {
            Form::Node(body) => body,
            Form::Stand(body) => body,
        }
    }
}

impl<'a> Through<'a> {
    pub fn new(written: Written<'a>, kept: &'a Kept) -> Through<'a> {
        Through {
            written,
            kept,
            standing: Kept::default(),
            stand_ins: RefCell::default(),
            replacing: RefCell::default(),
            moving: RefCell::default(),
            scopes: Cell::new(0),
        }
    }

    fn stand_in(id: NodeId) -> Option<usize> {
        let at = u32::MAX - id.0;
        (at < u32::MAX / 2).then_some(at as usize)
    }

    fn form(&self, id: NodeId) -> Option<(Form<'a>, Origin)> {
        match Through::stand_in(id) {
            Some(at) => {
                let body = Rc::clone(self.stand_ins.borrow().get(at)?);
                Some((Form::Stand(body), Origin::UNKNOWN))
            }
            None => (self.written)(id).map(|(body, origin)| (Form::Node(body), origin)),
        }
    }

    fn kept(&self, id: NodeId) -> &Kept {
        match Through::stand_in(id) {
            Some(_) => &self.standing,
            None => self.kept,
        }
    }

    /// A node of its own standing for `body`, read through like any other.
    pub fn stand(&self, body: Body) -> NodeId {
        let mut held = self.stand_ins.borrow_mut();
        held.push(Rc::new(body));
        NodeId(u32::MAX - (held.len() - 1) as u32)
    }

    /// `with` handed the written form `id` names, where it names one.
    pub fn read<R>(&self, id: NodeId, with: impl FnOnce(&Body) -> R) -> Option<R> {
        self.form(id).map(|(form, _)| with(&form))
    }

    pub fn stands(id: NodeId) -> bool {
        Through::stand_in(id).is_some()
    }
}

fn once<K: std::hash::Hash + Eq, V: Clone>(
    table: &Each<K, V>,
    key: K,
    of: impl FnOnce() -> V,
) -> V {
    if let Some(held) = table.borrow().get(&key) {
        return held.clone();
    }
    let found = of();
    table.borrow_mut().insert(key, found.clone());
    found
}

impl Reads for Through<'_> {
    fn lowered(&self, id: NodeId, origin: Origin, var: Var) -> Result<SpectralSum, Left> {
        let Some((form, own)) = self.form(id) else {
            return Opaque.lowered(id, origin, var);
        };
        once(&self.kept(id).lowered, (id, var), || {
            lower(&form, own, var, self)
        })
    }

    fn polynomial(&self, id: NodeId, reading: Reading) -> Option<Vec<Coeff>> {
        let (form, _) = self.form(id)?;
        once(&self.kept(id).polynomials, (id, reading), || {
            polynomial_read(&form, reading, self)
        })
    }

    fn timeless(&self, id: NodeId) -> bool {
        let Some((form, _)) = self.form(id) else {
            return Opaque.timeless(id);
        };
        once(&self.kept(id).timeless, id, || timeless(&form, self))
    }

    fn key_moves(&self, id: NodeId) -> bool {
        let Some((form, _)) = self.form(id) else {
            return Opaque.key_moves(id);
        };
        once(&self.kept(id).key_moves, id, || key_moves_read(&form, self))
    }

    fn mentions_line(&self, id: NodeId) -> bool {
        let Some((form, _)) = self.form(id) else {
            return Opaque.mentions_line(id);
        };
        once(&self.kept(id).mentions_line, id, || {
            crate::series::mentions_line_read(&form, self)
        })
    }

    fn axis(&self, id: NodeId) -> Option<Axis> {
        let (form, _) = self.form(id)?;
        Some(once(&self.kept(id).axes, id, || {
            crate::affine::axis_read(&form, &Within(self), self)
        }))
    }

    fn line(&self, id: NodeId) -> Option<Line> {
        let (form, _) = self.form(id)?;
        once(&self.kept(id).lines, id, || kink::line(&form, self))
    }

    fn kink(&self, id: NodeId) -> Option<(Body, Crossing)> {
        let (form, _) = self.form(id)?;
        once(&self.kept(id).kinks, id, || kink::first(&form, self))
    }

    fn replaced(&self, id: NodeId, scope: usize, kink: &Body, with: &Body) -> Option<NodeId> {
        let (form, _) = self.form(id)?;
        if let Some(held) = self.replacing.borrow().get(&(id, scope)) {
            return *held;
        }
        let rewritten = match &*form == kink {
            true => Some(with.clone()),
            false => kink::in_time(&form)
                .then(|| map_children(&form, |p| kink::replaced_part(p, (scope, kink, with), self)))
                .filter(|body| body != &*form),
        };
        let stand = rewritten.map(|body| self.stand(body));
        self.replacing.borrow_mut().insert((id, scope), stand);
        stand
    }

    fn moved(&self, id: NodeId, at: &Body) -> Option<NodeId> {
        let held = self.moving.borrow().get(&id).and_then(|read| {
            read.iter()
                .find_map(|(time, stand)| (time == at).then_some(*stand))
        });
        if let Some(held) = held {
            return held;
        }
        let (form, _) = self.form(id)?;
        let moved = read_at_with(&form, at, &|node| self.moved(node, at));
        let stand = Some(self.stand(moved));
        let mut moving = self.moving.borrow_mut();
        moving.entry(id).or_default().push((at.clone(), stand));
        stand
    }

    fn stand_for(&self, body: Body) -> Option<NodeId> {
        Some(self.stand(body))
    }

    fn scope(&self) -> usize {
        self.scopes.set(self.scopes.get() + 1);
        self.scopes.get()
    }

    fn view(&self, id: NodeId, read: &Read) -> Option<Form<'_>> {
        let (form, _) = self.form(id)?;
        let key = match read {
            Read::As => return Some(form),
            Read::At(at) => View::At(hash_time(at)),
            Read::By(by) => View::By(by.to_bits()),
        };
        let views = &self.kept(id).views;
        if let Some(held) = views.borrow().get(&(id, key)) {
            return Some(Form::Stand(Rc::clone(held)));
        }
        let seen = Rc::new(match read {
            Read::As => unreachable!("a form read as written is handed out as it stands"),
            Read::At(at) => read_at(&form, at),
            Read::By(by) => shift_line(&form, *by),
        });
        views.borrow_mut().insert((id, key), Rc::clone(&seen));
        Some(Form::Stand(seen))
    }

    fn growth(&self, id: NodeId, at: &Body, k: IndexId) -> Option<IndexGrowth> {
        let (form, _) = self.form(id)?;
        Some(once(&self.kept(id).growths, (id, k, hash_time(at)), || {
            crate::series::growth(&read_at(&form, at), k, &Within(self))
        }))
    }
}

/// Types nothing a reader reads through: a ref sits where its form does, and no parameter
/// reaches a form read through.
struct Within<'r>(&'r dyn Reads);

impl crate::env::Env for Within<'_> {
    fn node(&self, _: NodeId) -> crate::ty::Ty {
        crate::ty::Ty::form(Var::T, false, crate::ty::Codomain::Complex)
    }

    fn param(&self, _: crate::env::ParamId) -> crate::ty::Ty {
        crate::ty::Ty::form(Var::T, false, crate::ty::Codomain::Complex)
    }

    fn reads(&self) -> &dyn Reads {
        self.0
    }
}

/// `f` as a reader of its shape sees it: a ref is the form it names, each level kept per node
/// and time as the reader walks into it.
pub fn looked<'b>(f: &'b Body, reads: &'b dyn Reads) -> Seen<'b> {
    let mut seen = Seen::Here(f);
    loop {
        let read = match &*seen {
            Body::Node(id) => Some((*id, Read::As)),
            Body::Warp { at, of } => match &*of.body {
                Body::Node(id) => Some((*id, Read::At(&at.body))),
                _ => None,
            },
            Body::Shift { by, of } => match &*of.body {
                Body::Node(id) => Some((*id, Read::By(*by))),
                _ => None,
            },
            _ => None,
        };
        let Some(form) = read.and_then(|(id, read)| reads.view(id, &read)) else {
            return seen;
        };
        seen = Seen::Form(form);
    }
}

/// A body as a reader sees it: written where it stands, or the form a ref there names.
pub enum Seen<'b> {
    Here(&'b Body),
    Form(Form<'b>),
}

impl Deref for Seen<'_> {
    type Target = Body;

    fn deref(&self) -> &Body {
        match self {
            Seen::Here(body) => body,
            Seen::Form(form) => form,
        }
    }
}
