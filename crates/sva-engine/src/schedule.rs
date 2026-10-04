// Concern: orders nodes dependencies-first and picks which a reading materializes | Non-concern: what a held node contains (render.rs), reading one (refs.rs) | IO: (Instances, asks) -> Schedule

use std::collections::{BTreeSet, HashMap};

use sva_formula::{Held, NodeId, Var};

use crate::cast::Cast;
use crate::error::EngineError;
use crate::instantiate::Instances;
use crate::query::Ask;
use crate::typing::{Typing, Value, When};

/// One dependencies-first walk: the groups, and which are loops.
pub struct Order<'i> {
    pub groups: Vec<Vec<String>>,
    inst: &'i Instances,
}

impl<'i> Order<'i> {
    pub fn deps(&self, path: &str) -> &'i [String] {
        self.inst.deps(path)
    }

    /// A group of one not reading itself is no loop; any other is.
    pub fn is_loop(&self, group: &[String]) -> bool {
        is_loop(self.inst, group)
    }
}

pub(crate) fn is_loop(inst: &Instances, group: &[String]) -> bool {
    match group {
        [only] => inst.deps(only).iter().any(|d| d == only),
        _ => true,
    }
}

struct Frame {
    node: String,
    refs: Vec<String>,
    idx: usize,
}

/// A node two roots reach is grouped once, whichever reached it first.
pub fn schedule_from<'i>(inst: &'i Instances, roots: &[String]) -> Result<Order<'i>, EngineError> {
    let mut walk = Walk {
        inst,
        within: &|_| true,
        index: HashMap::new(),
        low: HashMap::new(),
        open: Vec::new(),
        next: 0,
        groups: Vec::new(),
    };
    for root in roots {
        if !inst.holds(root) {
            return Err(EngineError::UnknownNode(root.to_string()));
        }
        walk.from(root);
    }
    Ok(Order {
        groups: walk.groups,
        inst,
    })
}

/// The groups of `region`, dependencies first.
pub(crate) fn grouped(
    inst: &Instances,
    starts: &[String],
    region: &dyn Fn(&str) -> bool,
) -> Vec<Vec<String>> {
    let mut walk = Walk {
        inst,
        within: region,
        index: HashMap::new(),
        low: HashMap::new(),
        open: Vec::new(),
        next: 0,
        groups: Vec::new(),
    };
    for start in starts.iter().filter(|s| region(s)) {
        walk.from(start);
    }
    walk.groups
}

struct Walk<'a> {
    inst: &'a Instances,
    within: &'a dyn Fn(&str) -> bool,
    index: HashMap<String, usize>,
    low: HashMap<String, usize>,
    open: Vec<String>,
    next: usize,
    groups: Vec<Vec<String>>,
}

impl Walk<'_> {
    fn reads(&self, node: &str) -> Vec<String> {
        let reads = self.inst.deps(node).iter();
        reads.filter(|read| (self.within)(read)).cloned().collect()
    }

    fn from(&mut self, root: &str) {
        if self.index.contains_key(root) {
            return;
        }
        self.index.insert(root.to_string(), self.next);
        self.low.insert(root.to_string(), self.next);
        self.next += 1;
        self.open.push(root.to_string());
        let seed = self.reads(root);
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
                        let refs = self.reads(&target);
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
    }
}

/// What a render has to hold, and what it can leave as a closed form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schedule {
    /// What a reading holds for itself, the root among them.
    pub wanted: Vec<NodeId>,
    /// The nodes a reading asked a closed form of, none already held.
    pub compose: Vec<NodeId>,
}

/// A closed form is held as samples only under a buffer reading, a ledger, or the root.
pub fn plan(typing: &Typing, root: NodeId, asks: &[Ask]) -> Schedule {
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
    compose.retain(|id| !wanted.contains(id));
    Schedule {
        wanted: wanted.into_iter().collect(),
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
        while let Some(held) = level.pop() {
            for operand in read_operands(typing, held) {
                if !seen.insert(operand) {
                    continue;
                }
                wanted.insert(operand);
                match typing.name(operand) == typing.name(held) {
                    true => level.push(operand),
                    false => next.push(operand),
                }
            }
        }
        level = next;
    }
}

/// A node reading its own output is one program, whatever its width: a component on its own
/// would read the loop at its own width.
pub(crate) fn holds_self(typing: &Typing, id: NodeId, seen: &mut BTreeSet<NodeId>) -> bool {
    if !seen.insert(id) {
        return false;
    }
    match typing.value(id) {
        Value::SelfAt { .. } => true,
        Value::Cast(Cast::Sample, _) | Value::Read { .. } => false,
        Value::Cast(_, source) => holds_self(typing, *source, seen),
        Value::Op { args, .. } => args.iter().any(|a| holds_self(typing, *a, seen)),
        Value::Filter {
            x, cutoff, q, gain, ..
        } => [x, cutoff, q, gain]
            .into_iter()
            .any(|operand| holds_self(typing, *operand, seen)),
        Value::Solver { varying, .. } => varying.iter().any(|(_, a)| holds_self(typing, *a, seen)),
        Value::ClosedForm(_) | Value::Noise(_) => false,
    }
}

/// Which nodes under `id` a render has to hold before it can hold `id` itself: the buffers its
/// program reads, an operation, filter or read under it being part of that program.
pub(crate) fn materialized_operands(typing: &Typing, id: NodeId) -> Vec<NodeId> {
    let sampled = |set: Vec<NodeId>| -> Vec<NodeId> {
        let mut out = Vec::new();
        for op in set {
            if typing.ty(op).is_closed_form() {
                continue;
            }
            let inlined = matches!(
                typing.value(op),
                Value::Op { .. } | Value::Filter { .. } | Value::Read { .. }
            );
            match inlined || holds_self(typing, op, &mut BTreeSet::new()) {
                true => out.extend(materialized_operands(typing, op)),
                false => out.push(op),
            }
        }
        out
    };
    match typing.value(id) {
        Value::ClosedForm(_) | Value::Noise(_) => Vec::new(),
        Value::SelfAt { at, .. } => sampled(at.moving()),
        Value::Solver { varying, .. } => sampled(varying.iter().map(|(_, a)| *a).collect()),
        Value::Cast(Cast::Sample, source) => vec![*source],
        Value::Read { source, at, .. } => {
            let mut out = match (at, anywhere(typing, *source)) {
                (When::Moving(_) | When::Step(_), true) => Vec::new(),
                _ => vec![*source],
            };
            out.extend(sampled(at.moving()));
            out
        }
        Value::Cast(_, source) => sampled(vec![*source]),
        Value::Op { args, .. } => sampled(args.clone()),
        Value::Filter {
            x, cutoff, q, gain, ..
        } => sampled(vec![*x, *cutoff, *q, *gain]),
    }
}

/// Whether a read of `id` has a value at any instant, stored or not.
pub(crate) fn anywhere(typing: &Typing, id: NodeId) -> bool {
    match typing.value(id) {
        Value::Noise(_) => true,
        Value::Cast(Cast::Sample, of) => typing.ty(*of).held == Held::Form(Var::T),
        Value::ClosedForm(form) => form.var == Var::T,
        _ => false,
    }
}

/// Every ref one node reads.
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
        Value::Solver { varying, .. } => {
            for (_, arg) in varying {
                read_through(typing, *arg, seen, out);
            }
        }
        Value::SelfAt { .. } | Value::Noise(_) => {}
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
