// Concern: counts what a render or a stream costs in operations, per node, from the schedule alone | Non-concern: running any of it (render/) | IO: (&Render) -> Tree, per-sample price, Work

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::{Body, Held, NodeId};

use crate::render::Render;
use crate::{refs, schedule};

use sva_samples::collapse::plan;

/// `subtree` is what the holder pays for this ref; `shared` marks a row priced net of a tree
/// an earlier row already carried.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub depth: usize,
    pub node: String,
    pub own: u128,
    pub subtree: u128,
    pub percent: f64,
    pub route: &'static str,
    pub shared: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tree {
    pub total: u128,
    pub budget: u128,
    pub rows: Vec<Row>,
}

struct Node {
    id: NodeId,
    subtree: u128,
    own: u128,
    route: &'static str,
    shared: bool,
    children: Vec<Node>,
}

/// A row under this share of the whole folds into its level's `others`, unless it is `shared`.
const FOLD_PERCENT: u128 = 1;

/// A child this close to its parent restates it, and is skipped where it owns nothing.
const PASS_THROUGH_PERCENT: u128 = 99;

pub fn tree(render: &Render) -> Tree {
    tree_at(render, render.root)
}

/// A closed form's refs are inlined into its own collapse and are never on the materialize
/// list, so summing that list counts nothing twice.
pub fn total(render: &Render) -> u128 {
    match render.schedule.materialize.as_slice() {
        [] => costed(render, render.root).0,
        held => held.iter().map(|id| costed(render, *id).0).sum(),
    }
}

pub fn tree_at(render: &Render, from: NodeId) -> Tree {
    let mut walked = BTreeMap::from([(from, None)]);
    let root = grow(render, from, &[], &mut walked);
    let total = root.subtree;
    let mut rows = Vec::new();
    emit(&root, render, 0, total, &mut rows);
    Tree {
        total,
        budget: render.config.flop_budget,
        rows,
    }
}

/// The root restates the whole, so the refusal names the costliest row under it.
pub fn dominating(tree: &Tree) -> Option<&Row> {
    let root = tree.rows.first()?;
    tree.rows
        .iter()
        .skip(1)
        .filter(|row| row.node != root.node)
        .max_by_key(|row| row.subtree)
        .or(Some(root))
}

/// `chain` is the refs walked from `base`, the nearest node holding a buffer of its own.
fn grow(render: &Render, base: NodeId, chain: &[NodeId], walked: &mut Walked) -> Node {
    let id = chain.last().copied().unwrap_or(base);
    let (subtree, route, shared) = priced(render, base, chain, walked);
    let separate = schedule::materialized_operands(&render.tys, id);
    let reached: Vec<NodeId> = schedule::read_operands(&render.tys, id)
        .into_iter()
        .filter(|child| walked.insert(*child, None).is_none())
        .collect();
    let children: Vec<Node> = reached
        .into_iter()
        .map(|child| match separate.contains(&child) {
            true => grow(render, child, &[], walked),
            false => grow(render, base, &[chain, &[child]].concat(), walked),
        })
        .collect();
    // An inlined ref is already in this node's count; a materialized one is a buffer beside it.
    let (inlined, beside): (Vec<&Node>, Vec<&Node>) =
        children.iter().partition(|c| !separate.contains(&c.id));
    let inlined: u128 = inlined.iter().map(|c| c.subtree).sum();
    let beside: u128 = beside.iter().map(|c| c.subtree).sum();
    let held = Node {
        id,
        subtree: subtree + beside,
        own: subtree.saturating_sub(inlined),
        route,
        shared,
        children,
    };
    walked.insert(id, Some(held.subtree));
    held
}

/// Every node the walk reached, and what its row came to once it had one.
type Walked = BTreeMap<NodeId, Option<u128>>;

fn priced(
    render: &Render,
    base: NodeId,
    chain: &[NodeId],
    walked: &Walked,
) -> (u128, &'static str, bool) {
    match chain.last() {
        None => {
            let (cost, route) = costed(render, base);
            (cost, route, false)
        }
        Some(id) => read_as(render, base, chain, walked).unwrap_or_else(|| {
            let mut carried = Carried::beside(walked, *id);
            let (cost, route) = costed_under(render, *id, &mut carried);
            (cost, route, carried.shared)
        }),
    }
}

/// The holder's closed form with every other ref silenced: this one at the holder's own offset.
fn read_as(
    render: &Render,
    base: NodeId,
    chain: &[NodeId],
    walked: &Walked,
) -> Option<(u128, &'static str, bool)> {
    let crate::typing::Value::ClosedForm(form) = render.tys.value(base) else {
        return None;
    };
    let len = render.config.horizon.len(render.config.rate).ok()?;
    let (rate, horizon) = (render.config.rate, render.config.horizon);
    let profile = &render.config.profile;
    let body = refs::fold_constants(&render.tys, &along(render, &form.body, chain)?);
    let (carried, shared) = carried_already(&body, walked, chain);
    let plan = match refs::spectral_sum_of_body(&render.tys, base, &body, form.var) {
        Ok(sum) => plan::of(&sum, rate, horizon, profile, len).ok()?,
        Err(_) => {
            let written = sva_formula::ClosedForm {
                var: form.var,
                body: refs::substituted_body(&render.tys, base, &body)?,
                origin: form.origin,
            };
            plan::of_written(&written, rate, horizon, profile, len).ok()?
        }
    };
    Some((
        plan.flops(len).saturating_sub(carried),
        plan.rule().as_str(),
        shared,
    ))
}

fn carried_already(body: &Body, walked: &Walked, chain: &[NodeId]) -> (u128, bool) {
    if let Body::Node(id) = body
        && !chain.contains(id)
    {
        return match walked.get(id) {
            Some(Some(paid)) => (*paid, true),
            _ => (0, false),
        };
    }
    sva_formula::closed_form::children(body)
        .iter()
        .fold((0, false), |(sum, held), part| {
            let (paid, found) = carried_already(&part.body, walked, chain);
            (sum + paid, held || found)
        })
}

fn along(render: &Render, body: &sva_formula::Body, chain: &[NodeId]) -> Option<Body> {
    let Some((next, rest)) = chain.split_first() else {
        return Some(body.clone());
    };
    match body {
        Body::Node(id) if id == next => match render.tys.value(*id) {
            crate::typing::Value::ClosedForm(form) => along(render, &form.body, rest),
            _ => rest.is_empty().then(|| body.clone()),
        },
        Body::Node(_) => Some(Body::Const(sva_formula::C64::ZERO)),
        other => {
            let mut held = true;
            let out = sva_formula::closed_form::map_children(other, |part| {
                match along(render, &part.body, chain) {
                    Some(body) => sva_formula::Part::new(part.origin, body),
                    None => {
                        held = false;
                        part.clone()
                    }
                }
            });
            held.then_some(out)
        }
    }
}

/// The trees a price already carries: a tree two reads reach is charged under the first of them.
struct Carried {
    paid: BTreeSet<NodeId>,
    shared: bool,
}

impl Carried {
    fn of(id: NodeId) -> Self {
        Carried {
            paid: BTreeSet::from([id]),
            shared: false,
        }
    }

    fn beside(walked: &Walked, id: NodeId) -> Self {
        let mut held = Carried::of(id);
        held.paid.extend(
            walked
                .iter()
                .filter(|(_, row)| row.is_some())
                .map(|(n, _)| *n),
        );
        held.paid.insert(id);
        held
    }

    fn opens(&mut self, read: NodeId) -> bool {
        let fresh = self.paid.insert(read);
        self.shared |= !fresh;
        fresh
    }
}

fn costed(render: &Render, id: NodeId) -> (u128, &'static str) {
    costed_under(render, id, &mut Carried::of(id))
}

fn costed_under(render: &Render, id: NodeId, paid: &mut Carried) -> (u128, &'static str) {
    let Ok(len) = render.config.horizon.len(render.config.rate) else {
        return (0, "no horizon");
    };
    costed_at(render, id, len, paid)
}

fn costed_at(render: &Render, id: NodeId, len: usize, paid: &mut Carried) -> (u128, &'static str) {
    match render.tys.ty(id).held {
        Held::Frames => (frames_flops(render, id, len), "short-time transform"),
        Held::Sampled => (ops_of(render, id) as u128 * len as u128, "sampled program"),
        _ => closed_form_flops(render, id, len, paid),
    }
}

fn closed_form_flops(
    render: &Render,
    id: NodeId,
    len: usize,
    paid: &mut Carried,
) -> (u128, &'static str) {
    let var = render.tys.var(id);
    let (rate, horizon) = (render.config.rate, render.config.horizon);
    let profile = &render.config.profile;
    let sum = refs::spectral_sum_of(&render.tys, id, var)
        .ok()
        .and_then(|sum| sva_samples::collapse::plan::of(&sum, rate, horizon, profile, len).ok());
    // A form with no spectral sum takes the written rows, which a sum splits addend by addend.
    let plan = sum.or_else(|| {
        let form = written_closed_form(render, id)?;
        sva_samples::collapse::plan::of_written(&form, rate, horizon, profile, len).ok()
    });
    match plan {
        Some(plan) => (
            plan.flops(len) + scored(render, id, &plan, len),
            plan.rule().as_str(),
        ),
        None => (
            pointwise_flops(render, id, len, paid),
            sva_samples::Rule::PointSampled.as_str(),
        ),
    }
}

/// No row of the table takes this node, so every instant walks its own tree: the written body, or
/// the operation over each node it reads, each of those at its own price.
fn pointwise_flops(render: &Render, id: NodeId, len: usize, paid: &mut Carried) -> u128 {
    let own = match render.tys.value(id) {
        crate::typing::Value::ClosedForm(form) => plan::point_nodes(&form.body),
        _ => ops_of(render, id),
    };
    schedule::read_operands(&render.tys, id).into_iter().fold(
        own as u128 * len as u128,
        |sum, read| match paid.opens(read) {
            true => sum + costed_at(render, read, len, paid).0,
            false => sum,
        },
    )
}

/// One streamed sample of a node no row takes, priced as a whole render prices one sample of
/// it: a sampled program's operations, or a pointwise tree and the forms it reads.
pub(crate) fn per_sample(render: &Render, id: NodeId) -> u128 {
    match render.tys.ty(id).held {
        Held::Sampled => ops_of(render, id) as u128,
        _ => pointwise_flops(render, id, 1, &mut Carried::of(id)),
    }
}

/// What a stream or a render did, counted exactly and alike on every machine: the samples
/// written, the silence proofs run, the price of the work, and the lines and atoms summed
/// where they are counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    pub samples: u64,
    pub proofs: u64,
    pub priced_flops: u128,
    pub waves: Option<u128>,
}

/// Two references, because the render takes two: the label's, and the reading's own.
fn scored(render: &Render, id: NodeId, plan: &plan::Plan, len: usize) -> u128 {
    match render.alias_oversample(id) {
        Some(asked) => plan.alias_flops(len) + plan.alias_flops_at(len, asked as usize),
        None => 0,
    }
}

fn written_closed_form(render: &Render, id: NodeId) -> Option<sva_formula::ClosedForm> {
    match refs::resolve(&render.tys, id, 0, render.tys.ty(id).held) {
        Ok(refs::Read::Substitute(form)) => Some(*form),
        _ => None,
    }
}

fn frames_flops(render: &Render, id: NodeId, len: usize) -> u128 {
    let crate::typing::Value::Cast(crate::cast::Cast::Stft { window, hop }, _) =
        *render.tys.value(id)
    else {
        return 0;
    };
    let frames = len.div_ceil(hop.max(1)) as u128;
    frames * sva_samples::collapse::transform_flops(window.max(1))
}

/// The sampled tree under one node, stopping where an operand is a buffer of its own.
fn ops_of(render: &Render, id: NodeId) -> usize {
    let mut seen = BTreeSet::new();
    inner_ops(render, id, &mut seen)
}

fn inner_ops(render: &Render, id: NodeId, seen: &mut BTreeSet<NodeId>) -> usize {
    if !seen.insert(id) {
        return 1;
    }
    match render.tys.value(id) {
        crate::typing::Value::Op { args, .. } => {
            1 + args
                .iter()
                .map(|a| inner_ops(render, *a, seen))
                .sum::<usize>()
        }
        crate::typing::Value::Filter { x, .. } => 1 + inner_ops(render, *x, seen),
        _ => 1,
    }
}

fn emit(node: &Node, render: &Render, depth: usize, total: u128, rows: &mut Vec<Row>) {
    rows.push(Row {
        depth,
        node: render.tys.name(node.id).to_string(),
        own: node.own,
        subtree: node.subtree,
        percent: percent(node.subtree, total),
        route: node.route,
        shared: node.shared,
    });
    children(node, render, depth + 1, total, rows);
}

fn children(node: &Node, render: &Render, depth: usize, total: u128, rows: &mut Vec<Row>) {
    let mut held: Vec<&Node> = node.children.iter().collect();
    held.sort_by_key(|c| std::cmp::Reverse(c.subtree));
    let (shown, folded): (Vec<&Node>, Vec<&Node>) = held
        .into_iter()
        .partition(|c| c.shared || c.subtree * 100 >= total * FOLD_PERCENT);
    for child in shown {
        let restates = child.subtree * 100 >= node.subtree * PASS_THROUGH_PERCENT
            && child.own * 100 < total * FOLD_PERCENT;
        match restates && !child.children.is_empty() {
            true => children(child, render, depth, total, rows),
            false => emit(child, render, depth, total, rows),
        }
    }
    if !folded.is_empty() {
        let under: u128 = folded.iter().map(|c| c.subtree).sum();
        rows.push(Row {
            depth,
            node: format!("{} others", folded.len()),
            own: under,
            subtree: under,
            percent: percent(under, total),
            route: "folded",
            shared: false,
        });
    }
}

fn percent(part: u128, whole: u128) -> f64 {
    match whole {
        0 => 0.0,
        _ => 100.0 * part as f64 / whole as f64,
    }
}
