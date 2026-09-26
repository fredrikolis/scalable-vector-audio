// Concern: orders nodes dependencies-first and picks which a reading materializes | Non-concern: what a held node contains (render.rs), reading one (refs.rs) | IO: (Instances, asks) -> Schedule

use std::collections::{BTreeMap, BTreeSet, HashMap};

use sva_ast::{Arg, Expr};
use sva_formula::NodeId;

use crate::cast::Cast;
use crate::error::EngineError;
use crate::instantiate::{Cx, Instances, Node};
use crate::query::Ask;
use crate::typing::{Typing, Value};

/// One-hop instance names. `self(...)` is a same-node read, never an edge.
fn direct_refs(inst: &Instances, e: &Expr, cx: Cx, out: &mut Vec<String>) {
    if inst
        .follow(e, cx, |e2, cx2| direct_refs(inst, e2, cx2, out))
        .is_some()
    {
        return;
    }
    match inst.node(e, cx) {
        Node::Lit(_) | Node::Name(_) => {}
        Node::Bin(_, l, r) => {
            direct_refs(inst, l, cx, out);
            direct_refs(inst, r, cx, out);
        }
        Node::Call { args, .. } => {
            for a in args {
                let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                direct_refs(inst, x, cx, out);
            }
        }
        Node::Read { path, arg, .. } => {
            out.push(path.to_string());
            direct_refs(inst, arg, cx, out);
        }
        Node::Own { arg, .. } => direct_refs(inst, arg, cx, out),
    }
}

/// One dependencies-first walk: the groups, what each node reads, and which are loops.
pub struct Order {
    pub groups: Vec<Vec<String>>,
    deps: BTreeMap<String, Vec<String>>,
}

impl Order {
    pub fn deps(&self, path: &str) -> &[String] {
        self.deps.get(path).map_or(&[], Vec::as_slice)
    }

    /// A group of one whose node does not ref itself is an ordinary node; anything else is a loop.
    pub fn is_loop(&self, group: &[String]) -> bool {
        match group {
            [only] => self.deps(only).iter().any(|d| d == only),
            _ => true,
        }
    }
}

pub fn direct_deps(inst: &Instances, path: &str) -> Result<Vec<String>, EngineError> {
    let (e, cx) = inst
        .at(path)
        .ok_or_else(|| EngineError::UnknownNode(path.to_string()))?;
    let mut out = Vec::new();
    direct_refs(inst, e, cx, &mut out);
    out.sort();
    out.dedup();
    for target in &out {
        if !inst.holds(target) {
            return Err(EngineError::UnknownNode(target.clone()));
        }
    }
    Ok(out)
}

struct Frame {
    node: String,
    refs: Vec<String>,
    idx: usize,
}

/// A node two roots reach is grouped once, whichever reached it first.
pub fn schedule_from(inst: &Instances, roots: &[String]) -> Result<Order, EngineError> {
    let mut walk = Walk {
        inst,
        deps: BTreeMap::new(),
        index: HashMap::new(),
        low: HashMap::new(),
        open: Vec::new(),
        next: 0,
        groups: Vec::new(),
    };
    for root in roots {
        walk.from(root)?;
    }
    Ok(Order {
        groups: walk.groups,
        deps: walk.deps,
    })
}

struct Walk<'a> {
    inst: &'a Instances<'a>,
    deps: BTreeMap<String, Vec<String>>,
    index: HashMap<String, usize>,
    low: HashMap<String, usize>,
    open: Vec<String>,
    next: usize,
    groups: Vec<Vec<String>>,
}

impl Walk<'_> {
    fn from(&mut self, root: &str) -> Result<(), EngineError> {
        if !self.inst.holds(root) {
            return Err(EngineError::UnknownNode(root.to_string()));
        }
        if self.index.contains_key(root) {
            return Ok(());
        }
        self.index.insert(root.to_string(), self.next);
        self.low.insert(root.to_string(), self.next);
        self.next += 1;
        self.open.push(root.to_string());
        let seed = direct_deps(self.inst, root)?;
        self.deps.insert(root.to_string(), seed.clone());
        let mut stack = vec![Frame {
            node: root.to_string(),
            refs: seed,
            idx: 0,
        }];

        while let Some(frame) = stack.last_mut() {
            if frame.idx < frame.refs.len() {
                let target = frame.refs[frame.idx].clone();
                let node = frame.node.clone();
                frame.idx += 1;
                match self.index.get(&target).copied() {
                    None => {
                        self.index.insert(target.clone(), self.next);
                        self.low.insert(target.clone(), self.next);
                        self.next += 1;
                        self.open.push(target.clone());
                        let refs = direct_deps(self.inst, &target)?;
                        self.deps.insert(target.clone(), refs.clone());
                        stack.push(Frame {
                            node: target,
                            refs,
                            idx: 0,
                        });
                    }
                    Some(at) if self.open.contains(&target) => {
                        let mine = self.low[&node];
                        self.low.insert(node, mine.min(at));
                    }
                    Some(_) => {}
                }
                continue;
            }

            let node = frame.node.clone();
            let mine = self.low[&node];
            stack.pop();
            if let Some(parent) = stack.last() {
                let above = self.low[&parent.node];
                self.low.insert(parent.node.clone(), above.min(mine));
            }
            if mine == self.index[&node] {
                let at = self
                    .open
                    .iter()
                    .rposition(|n| *n == node)
                    .expect("a root of its group is still open");
                let mut group = self.open.split_off(at);
                group.sort();
                self.groups.push(group);
            }
        }
        Ok(())
    }
}

/// What a render has to hold, and what it can leave as a closed form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schedule {
    pub materialize: Vec<NodeId>,
    pub symbolic: Vec<NodeId>,
    /// The nodes a reading asked a closed form of, each named once however many asked
    /// and none of them already materialized.
    pub compose: Vec<NodeId>,
}

/// A closed form is materialized only under a buffer reading, a `sample(...)`, or the render root.
pub fn plan(typing: &Typing, order: &Order, root: NodeId, asks: &[Ask]) -> Schedule {
    let mut wanted: BTreeSet<NodeId> = BTreeSet::new();
    let mut compose: Vec<NodeId> = Vec::new();
    let audio = asks.is_empty();
    for ask in asks {
        let Some(id) = typing.id(&ask.node) else {
            continue;
        };
        // FORMAT 14.2: `bindings` and `arguments` are structural and `flops` counts; none composes.
        if matches!(
            ask.representation,
            crate::query::Representation::Bindings
                | crate::query::Representation::Arguments
                | crate::query::Representation::Flops
        ) {
            continue;
        }
        match ask.representation.consumes(typing.ty(id).is_closed_form()) {
            sva_samples::Consumes::ClosedForm if !compose.contains(&id) => compose.push(id),
            sva_samples::Consumes::ClosedForm => {}
            _ => {
                wanted.insert(id);
            }
        }
        if let crate::query::Representation::Ledger { depth } = ask.representation {
            attributed(typing, id, depth, &mut wanted);
        }
    }
    if audio || !wanted.is_empty() {
        wanted.insert(root);
    }

    let mut reached: BTreeSet<NodeId> = BTreeSet::new();
    let mut work: Vec<NodeId> = wanted.iter().copied().collect();
    while let Some(id) = work.pop() {
        if !reached.insert(id) {
            continue;
        }
        work.extend(materialized_operands(typing, id));
    }

    let held: Vec<NodeId> = order
        .groups
        .concat()
        .iter()
        .filter_map(|path| typing.id(path))
        .collect();
    let mut materialize: Vec<NodeId> = Vec::new();
    let mut seen: BTreeSet<NodeId> = BTreeSet::new();
    for id in held {
        for member in dependencies_first(typing, id, &mut seen) {
            if reached.contains(&member) {
                materialize.push(member);
            }
        }
    }
    let symbolic = typing
        .paths()
        .map(|(_, id)| id)
        .filter(|id| !materialize.contains(id))
        .collect();
    compose.retain(|id| !materialize.contains(id));
    Schedule {
        materialize,
        symbolic,
        compose,
    }
}

/// A ledger names every ref under its target, so each is a buffer of its own.
fn attributed(typing: &Typing, id: NodeId, depth: usize, wanted: &mut BTreeSet<NodeId>) {
    // Level by level, as the reading walks: a node is held at its shortest chain's depth.
    let mut seen = BTreeSet::from([id]);
    let mut level = vec![id];
    for _ in 0..depth {
        let mut next = Vec::new();
        for held in level {
            for operand in read_operands(typing, held) {
                if seen.insert(operand) {
                    wanted.insert(operand);
                    next.push(operand);
                }
            }
        }
        level = next;
    }
}

/// A node reading its own output is one program, whatever its width: a component lowered on
/// its own would read the loop at that component's width instead of the node's.
pub(crate) fn holds_self(typing: &Typing, id: NodeId, seen: &mut BTreeSet<NodeId>) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match typing.value(id) {
        Value::SelfAt(_) => true,
        Value::Cast(Cast::Sample, _) | Value::Read { .. } => false,
        Value::Cast(_, source) => holds_self(typing, *source, seen),
        Value::Op { args, .. } => args.iter().any(|a| holds_self(typing, *a, seen)),
        Value::Filter {
            x, cutoff, q, gain, ..
        } => [x, cutoff, q, gain]
            .into_iter()
            .any(|operand| holds_self(typing, *operand, seen)),
        Value::ClosedForm(_) | Value::Solver(_) | Value::Grid(_) => false,
    }
}

/// Which nodes under `id` a render has to hold before it can hold `id` itself.
pub(crate) fn materialized_operands(typing: &Typing, id: NodeId) -> Vec<NodeId> {
    let sampled = |set: Vec<NodeId>| -> Vec<NodeId> {
        let mut out = Vec::new();
        for op in set {
            if typing.ty(op).is_closed_form() || matches!(typing.value(op), Value::Grid(_)) {
                continue;
            }
            match holds_self(typing, op, &mut BTreeSet::new()) {
                true => out.extend(materialized_operands(typing, op)),
                false => out.push(op),
            }
        }
        out
    };
    match typing.value(id) {
        Value::ClosedForm(_) | Value::SelfAt(_) | Value::Solver(_) | Value::Grid(_) => Vec::new(),
        Value::Cast(Cast::Sample, source) => vec![*source],
        Value::Read { source, .. } => vec![*source],
        Value::Cast(_, source) => sampled(vec![*source]),
        Value::Op { args, .. } => sampled(args.clone()),
        Value::Filter {
            x, cutoff, q, gain, ..
        } => sampled(vec![*x, *cutoff, *q, *gain]),
    }
}

/// The held nodes two or more held nodes read: a value one reader alone needs is covered by
/// that reader's own.
pub(crate) fn forks(typing: &Typing, held: &[NodeId]) -> BTreeSet<NodeId> {
    let mut readers: BTreeMap<NodeId, usize> = BTreeMap::new();
    for id in held {
        let mut read = materialized_operands(typing, *id);
        read.sort_unstable();
        read.dedup();
        for operand in read {
            *readers.entry(operand).or_default() += 1;
        }
    }
    readers
        .into_iter()
        .filter(|(id, count)| *count >= 2 && held.contains(id))
        .map(|(id, _)| id)
        .collect()
}

/// Operands before the node, so a run never reads a buffer it has not filled.
pub(crate) fn dependencies_first(
    typing: &Typing,
    id: NodeId,
    seen: &mut BTreeSet<NodeId>,
) -> Vec<NodeId> {
    if !seen.insert(id) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for operand in materialized_operands(typing, id) {
        out.extend(dependencies_first(typing, operand, seen));
    }
    out.push(id);
    out
}

/// Every ref one node reads; `materialized_operands` answers a narrower one.
pub(crate) fn read_operands(typing: &Typing, id: NodeId) -> Vec<NodeId> {
    match typing.value(id) {
        Value::ClosedForm(form) => crate::refs::nodes_in(&form.body),
        _ if typing.ty(id).is_closed_form() => {
            let mut out = Vec::new();
            reads_under(typing, id, &mut BTreeSet::new(), &mut out);
            out.dedup();
            out
        }
        _ => materialized_operands(typing, id),
    }
}

/// A closed-form-typed node that holds no written one still reads refs through its operands.
fn reads_under(typing: &Typing, id: NodeId, seen: &mut BTreeSet<NodeId>, out: &mut Vec<NodeId>) {
    if !seen.insert(id) {
        return;
    }
    match typing.value(id) {
        Value::ClosedForm(form) => out.extend(crate::refs::nodes_in(&form.body)),
        Value::Read { source, .. } => out.push(*source),
        Value::Cast(_, source) => read_through(typing, *source, seen, out),
        Value::Op { args, .. } => {
            for arg in args {
                read_through(typing, *arg, seen, out);
            }
        }
        Value::Filter {
            x, cutoff, q, gain, ..
        } => {
            for operand in [x, cutoff, q, gain] {
                read_through(typing, *operand, seen, out);
            }
        }
        Value::SelfAt(_) | Value::Grid(_) | Value::Solver(_) => {}
    }
}

/// A written form under an operand is the term read, whatever transform sits between.
fn read_through(typing: &Typing, id: NodeId, seen: &mut BTreeSet<NodeId>, out: &mut Vec<NodeId>) {
    let Value::ClosedForm(_) = typing.value(id) else {
        return reads_under(typing, id, seen, out);
    };
    if seen.insert(id) {
        out.push(id);
    }
}
