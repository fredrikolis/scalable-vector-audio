// Concern: takes into a new typing the nodes a prior one lowered from unchanged sources | Non-concern: deciding which sources changed, lowering the rest | IO: (prior Typing, group) -> nodes taken

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

use sva_formula::closed_form::map_children;
use sva_formula::{Body, ClosedForm, NodeId, Origin, Part};

use super::{Node, Step, Typing, Value, When};
use crate::schedule::Order;
use crate::time::Grid;

/// What one group's lowering wrote into a typing: its nodes, and the origins they carry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) nodes: Range<u32>,
    pub(crate) origins: Range<u32>,
}

/// A typing a new one is built beside, and whether a node's source is the one it was typed from.
pub(crate) struct Prior<'p> {
    pub(crate) typing: &'p Typing,
    pub(crate) same: &'p dyn Fn(&str) -> bool,
}

/// What a new typing took from its prior: each node by its prior id, each origin by its prior
/// token, and the paths whose groups it took.
pub(crate) struct Carried {
    pub(crate) nodes: HashMap<NodeId, NodeId>,
    origins: HashMap<u32, u32>,
    paths: BTreeSet<String>,
    copies: HashMap<NodeId, (String, Grid)>,
}

impl Carried {
    pub(crate) fn over(prior: &Typing) -> Carried {
        Carried {
            nodes: HashMap::new(),
            origins: HashMap::new(),
            paths: BTreeSet::new(),
            copies: prior
                .copies
                .iter()
                .map(|(at, id)| (*id, at.clone()))
                .collect(),
        }
    }
}

impl Typing {
    /// Takes `group` as `prior` lowered it, where its sources are unchanged and every node it
    /// reads was taken too; false, taking nothing, where any is not.
    pub(crate) fn carry(
        &mut self,
        prior: &Prior<'_>,
        order: &Order,
        group: &[String],
        carried: &mut Carried,
    ) -> bool {
        let held = prior.typing;
        let Some(span) = group.first().and_then(|first| held.spans.get(first)) else {
            return false;
        };
        let unchanged = group
            .iter()
            .all(|path| (prior.same)(path) && held.spans.get(path) == Some(span));
        let reads = group.iter().flat_map(|path| order.deps(path));
        let taken = reads
            .into_iter()
            .all(|read| group.contains(read) || carried.paths.contains(read));
        let copied = held
            .copies
            .iter()
            .filter(|(_, id)| span.nodes.contains(&id.0));
        if !unchanged || !taken || copied.clone().any(|(at, _)| self.copies.contains_key(at)) {
            return false;
        }
        let (base, from) = (self.nodes.len() as u32, self.origins.len() as u32);
        let node = |old: NodeId| match span.nodes.contains(&old.0) {
            true => Some(NodeId(base + old.0 - span.nodes.start)),
            false => carried.nodes.get(&old).copied().or_else(|| {
                let at = carried.copies.get(&old)?;
                self.copies.get(at).copied()
            }),
        };
        let origin =
            |old: Origin| match (old == Origin::UNKNOWN, span.origins.contains(&old.token())) {
                (true, _) => Some(old),
                (_, true) => Some(Origin::new(from + old.token() - span.origins.start)),
                _ => carried.origins.get(&old.token()).map(|at| Origin::new(*at)),
            };
        let range = span.nodes.start as usize..span.nodes.end as usize;
        let Some(nodes) = held.nodes[range]
            .iter()
            .map(|n| remapped(n, &node, &origin))
            .collect::<Option<Vec<Node>>>()
        else {
            return false;
        };
        let Some(paths) = group
            .iter()
            .map(|path| Some((path.clone(), node(held.id(path)?)?)))
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        let copies: Vec<_> = copied.map(|(at, id)| (at.clone(), node(*id))).collect();
        for token in span.origins.clone() {
            let (site, at) = held.origins[token as usize];
            self.mark(&held.sites[site as usize], at);
            carried
                .origins
                .insert(token, from + token - span.origins.start);
        }
        for (old, taken) in span.nodes.clone().zip(nodes) {
            carried
                .nodes
                .insert(NodeId(old), NodeId(self.nodes.len() as u32));
            self.nodes.push(taken);
        }
        for (path, id) in paths {
            if let Some(arguments) = held.arguments.get(&path) {
                self.arguments.insert(path.clone(), arguments.clone());
            }
            self.by_path.insert(path.clone(), id);
            carried.paths.insert(path);
        }
        for (at, id) in copies {
            self.copies.insert(at, id.expect("a copy inside the span"));
        }
        true
    }
}

/// `node` with every node it reads and every origin it carries renamed; `None` where one has
/// no name.
fn remapped(
    node: &Node,
    id: &dyn Fn(NodeId) -> Option<NodeId>,
    origin: &dyn Fn(Origin) -> Option<Origin>,
) -> Option<Node> {
    let lost = Cell::new(false);
    let id = |old: NodeId| {
        id(old).unwrap_or_else(|| {
            lost.set(true);
            old
        })
    };
    let origin = |old: Origin| {
        origin(old).unwrap_or_else(|| {
            lost.set(true);
            old
        })
    };
    let when = |at: &When| match at {
        When::Moving(time) => When::Moving(id(*time)),
        When::Step(step) => When::Step(stepped(step, &id)),
        other => other.clone(),
    };
    let value = match &node.value {
        Value::ClosedForm(form) => Value::ClosedForm(ClosedForm {
            var: form.var,
            body: body(&form.body, &id, &origin),
            origin: origin(form.origin),
        }),
        Value::Cast(cast, of) => Value::Cast(*cast, id(*of)),
        Value::Op { name, args } => Value::Op {
            name: name.clone(),
            args: args.iter().map(|a| id(*a)).collect(),
        },
        Value::SelfAt { at } => Value::SelfAt { at: when(at) },
        Value::Read { source, at, site } => Value::Read {
            source: id(*source),
            at: when(at),
            site: origin(*site),
        },
        Value::Filter {
            shape,
            x,
            cutoff,
            q,
            gain,
        } => Value::Filter {
            shape: *shape,
            x: id(*x),
            cutoff: id(*cutoff),
            q: id(*q),
            gain: id(*gain),
        },
        Value::Solver { params, varying } => Value::Solver {
            params: params.clone(),
            varying: varying.iter().map(|(k, a)| (*k, id(*a))).collect(),
        },
        Value::Noise(_) | Value::Stored(_) => node.value.clone(),
    };
    let renamed = Node {
        name: node.name.clone(),
        ty: node.ty,
        var: node.var,
        value,
        grid: node.grid,
    };
    (!lost.get()).then_some(renamed)
}

fn stepped(step: &Step, id: &dyn Fn(NodeId) -> NodeId) -> Step {
    match step {
        Step::Index(index) => Step::Index(*index),
        Step::Nearest(time, round) => Step::Nearest(id(*time), *round),
        Step::Add(parts) => Step::Add(parts.iter().map(|p| stepped(p, id)).collect()),
        Step::Mul(parts) => Step::Mul(parts.iter().map(|p| stepped(p, id)).collect()),
        Step::Neg(part) => Step::Neg(Box::new(stepped(part, id))),
    }
}

fn body(at: &Body, id: &dyn Fn(NodeId) -> NodeId, origin: &dyn Fn(Origin) -> Origin) -> Body {
    match at {
        Body::Node(node) => Body::Node(id(*node)),
        other => map_children(other, |part| Part {
            origin: origin(part.origin),
            body: Box::new(body(&part.body, id, origin)),
        }),
    }
}
