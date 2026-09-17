// Concern: states one node's position in a composition — what it is built from and everyone who reads it | Non-concern: measuring it, which needs audio | IO: (&Graph, seeds, target) -> Traced

use std::collections::{BTreeMap, BTreeSet};

use sva_ast::Graph;

use crate::error::{BindingFault, EngineError};
use crate::instantiate::Instances;
use crate::schedule::{self as refs};

pub struct Up {
    pub node: String,
    pub expr: String,
    pub ty: String,
}

pub struct Traced {
    pub node: String,
    pub expr: String,
    /// The node's own type, spelled as FORMAT 3.1 names it, with its width.
    pub ty: String,
    /// What made a node discrete: `sp`, a finite-difference builtin, or a `sample(...)`.
    pub discrete: Option<String>,
    pub entry: Vec<String>,
    pub file: Option<String>,
    /// The members of the feedback loop this node sits in, if any.
    pub cycle: Option<Vec<String>>,
    pub down: Vec<String>,
    pub up: Vec<Up>,
}

/// No audio: structure alone, over the instances every root reaches.
pub fn trace(graph: &Graph, roots: &[String], target: &str) -> Result<Traced, EngineError> {
    // A bare file name is the file on its own terms, as `render` and `lint` read it.
    let seeded = (graph.defines(target) && !roots.iter().any(|r| r == target)).then(|| {
        let mut held = roots.to_vec();
        held.push(target.to_string());
        held
    });
    let (inst, entries) = match &seeded {
        None => crate::instantiate::from_roots(graph, roots)?,
        Some(held) => match crate::instantiate::from_roots(graph, held) {
            Ok(found) => found,
            // Own terms a caller must complete are none: the call sites' are what is left.
            Err(EngineError::Binding {
                fault: BindingFault::Unbound(..),
                ..
            }) => crate::instantiate::from_roots(graph, roots)?,
            Err(other) => return Err(other),
        },
    };
    let node = inst.instance_of(target)?;

    // The seeded root is no entry point; every other one is.
    let named = match entries.len() > roots.len() {
        true => entries[..entries.len() - 1].to_vec(),
        false => entries.clone(),
    };
    let scheduled = refs::schedule_from(&inst, &entries)?;
    let typing = crate::typing::infer_all(&inst, &scheduled)?;
    let groups = scheduled.groups.clone();
    let order: Vec<String> = groups.concat();

    let cycle = groups
        .iter()
        .filter(|g| scheduled.is_loop(g))
        .find(|g| g.len() > 1 && g.contains(&node))
        .map(|g| sorted(g.clone()));

    let mut down = scheduled.deps(&node).to_vec();
    if inst.reads_self(&node) {
        down.push(node.clone());
    }

    Ok(Traced {
        expr: expr_of(&inst, &node),
        ty: spelled(&typing, &node),
        discrete: typing
            .id(&node)
            .and_then(|id| sampled_leaf(&typing, id, &mut Vec::new())),
        entry: sorted(named),
        file: inst
            .origin(&node)
            .filter(|f| *f != node)
            .map(str::to_string),
        cycle,
        up: readers(&inst, &typing, &scheduled, &order, &node),
        down: sorted(down),
        node,
    })
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names.dedup();
    names
}

fn expr_of(inst: &Instances, path: &str) -> String {
    match inst.at(path) {
        Some((e, cx)) => inst.render(e, cx),
        None => String::new(),
    }
}

/// The type a trace prints beside a node, with the width where the value is wide.
fn spelled(typing: &crate::typing::Typing, path: &str) -> String {
    match typing.id(path) {
        Some(id) => crate::overload::describe(typing.ty(id)),
        None => String::new(),
    }
}

/// The leaf a sampled node owes its grid to: the one thing a type alone does not say.
fn sampled_leaf(
    typing: &crate::typing::Typing,
    id: sva_formula::NodeId,
    open: &mut Vec<sva_formula::NodeId>,
) -> Option<String> {
    if typing.ty(id).is_closed_form() || open.contains(&id) {
        return None;
    }
    open.push(id);
    let under = |set: Vec<sva_formula::NodeId>, open: &mut Vec<sva_formula::NodeId>| {
        set.into_iter()
            .find_map(|op| sampled_leaf(typing, op, open))
    };
    match typing.value(id) {
        crate::typing::Value::SelfAt(_) => Some("sp, a self-reference on the grid".to_string()),
        crate::typing::Value::Grid(_) => Some("sp, a duration on the grid".to_string()),
        crate::typing::Value::Solver(_) => Some("a finite-difference builtin".to_string()),
        crate::typing::Value::Cast(crate::cast::Cast::Sample, source) => {
            Some(format!("sample({})", typing.name(*source)))
        }
        crate::typing::Value::Cast(cast, source) => {
            under(vec![*source], open).or_else(|| Some(cast.name().to_string()))
        }
        crate::typing::Value::Read { source, at, .. } => under(vec![*source], open).or_else(|| {
            Some(match at {
                crate::offset::Offset::Steps(steps) => format!("a read {steps} samples back"),
                crate::offset::Offset::Secs(secs) => format!("a read {secs} seconds back"),
            })
        }),
        crate::typing::Value::Op { args, .. } => under(args.clone(), open),
        crate::typing::Value::Filter { x, .. } => under(vec![*x], open),
        crate::typing::Value::ClosedForm(_) => None,
    }
}

fn readers(
    inst: &Instances,
    typing: &crate::typing::Typing,
    scheduled: &refs::Order,
    order: &[String],
    node: &str,
) -> Vec<Up> {
    let mut above: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for path in order {
        for target in scheduled.deps(path) {
            if target != path {
                above
                    .entry(target.as_str())
                    .or_default()
                    .push(path.as_str());
            }
        }
    }
    let mut seen: BTreeSet<&str> = BTreeSet::from([node]);
    let mut work: Vec<&str> = vec![node];
    while let Some(at) = work.pop() {
        for reader in above.get(at).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.insert(reader) {
                work.push(reader);
            }
        }
    }
    seen.into_iter()
        .filter(|p| *p != node)
        .map(|p| Up {
            node: p.to_string(),
            expr: expr_of(inst, p),
            ty: spelled(typing, p),
        })
        .collect()
}
