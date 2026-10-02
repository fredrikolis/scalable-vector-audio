// Concern: reads each ref of a written form as the form it names, each answer kept per node | Non-concern: which refs a caller lets it read (sva-engine) | IO: (NodeId) -> each reading of its form

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Deref;
use std::rc::Rc;

use crate::affine::{Coeff, Reading, key_moves_read, polynomial_read};
use crate::closed_form::{Body, Part, Var, map_children};
use crate::env::NodeId;
use crate::origin::Origin;
use crate::refusal::{Factor, Left, LeftReason};
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
    fn line(&self, id: NodeId) -> Option<Line>;
    fn kink(&self, id: NodeId) -> Option<(Body, Crossing)>;
    /// A node standing for `id` with `kink` read as `with` throughout, where it reaches one.
    fn replaced(&self, id: NodeId, scope: usize, kink: &Body, with: &Body) -> Option<NodeId>;
    fn scope(&self) -> usize;
    /// `id`'s form written out whole, for a representation that holds a written term.
    fn written(&self, id: NodeId) -> Body;
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

    fn line(&self, _: NodeId) -> Option<Line> {
        None
    }

    fn kink(&self, _: NodeId) -> Option<(Body, Crossing)> {
        None
    }

    fn replaced(&self, _: NodeId, _: usize, _: &Body, _: &Body) -> Option<NodeId> {
        None
    }

    fn scope(&self) -> usize {
        0
    }

    fn written(&self, id: NodeId) -> Body {
        Body::Node(id)
    }
}

/// Each reading of each node, kept by whoever holds the nodes for as long as they hold.
#[derive(Debug, Default)]
pub struct Kept {
    lowered: Each<(NodeId, Var), Result<SpectralSum, Left>>,
    polynomials: Each<(NodeId, Reading), Option<Vec<Coeff>>>,
    timeless: Each<NodeId, bool>,
    key_moves: Each<NodeId, bool>,
    lines: Each<NodeId, Option<Line>>,
    kinks: Each<NodeId, Option<(Body, Crossing)>>,
}

type Each<K, V> = RefCell<HashMap<K, V>>;

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
    scopes: Cell<usize>,
}

enum Form<'a> {
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

    fn scope(&self) -> usize {
        self.scopes.set(self.scopes.get() + 1);
        self.scopes.get()
    }

    fn written(&self, id: NodeId) -> Body {
        match self.form(id) {
            Some((form, _)) => written_out(&form, self),
            None => Opaque.written(id),
        }
    }
}

/// `f` with every ref `reads` knows written in as the form it names.
pub fn written_out(f: &Body, reads: &dyn Reads) -> Body {
    match f {
        Body::Node(id) => reads.written(*id),
        other => map_children(other, |p| Part::new(p.origin, written_out(&p.body, reads))),
    }
}
