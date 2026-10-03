// Concern: marks the nodes volatile parameters reach and names each one's slot | Non-concern: the store, what a node computes | IO: (Graph, Instances, Typing, names) -> a slot per volatile node

use std::collections::{BTreeMap, BTreeSet, HashMap};

use sva_ast::{Expr, Graph, Literal};
use sva_formula::{Hash, NodeId};

use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::{Instances, ScopeId};
use crate::render::Render;
use crate::render::session::Typed;
use crate::typing::{Typing, Value};

/// The base of each volatile node's slot; a node absent here keeps every value it stores.
#[derive(Default)]
pub(super) struct Volatile {
    slots: BTreeMap<NodeId, Hash>,
    /// Why the knobs' values keep an entry each: the stand-in refused.
    pub(super) unslotted: Option<String>,
    /// Each node the stand-in typed.
    pub(super) typed: Vec<String>,
}

impl Volatile {
    pub(super) fn slot(&self, id: NodeId) -> Option<Hash> {
        self.slots.get(&id).copied()
    }
}

/// A node is volatile when its instance reads a volatile parameter, directly or through what a
/// caller bound, or anything it is built from is. Its slot is what it computes with every
/// volatile parameter at one stand-in, however far a knob moves.
pub(super) fn mark(
    (graph, inst): (&Graph, &Instances),
    held: &Render,
    (target, stand_in): (&str, &mut Typed),
) -> Result<Volatile, EngineError> {
    let (tys, config) = (&held.tys, &held.config);
    if config.volatile.is_empty() {
        return Ok(Volatile::default());
    }
    refuse_unbound(inst, &config.volatile, target)?;
    let mut reach = Reach {
        inst,
        names: &config.volatile,
        bound: HashMap::new(),
    };
    let instances: BTreeSet<&str> = inst.paths().filter(|p| reach.instance(p)).collect();
    let (paired, at) = match at_stand_in(graph, target, (config, stand_in)) {
        Ok(held) => held,
        Err(refused) => {
            return Ok(Volatile {
                unslotted: Some(refused.to_string()),
                ..Volatile::default()
            });
        }
    };
    let (mut marked, mut slots, mut unslotted) = (HashMap::new(), BTreeMap::new(), None);
    for (id, at) in pair((tys, held.root), (paired, at)) {
        if !volatile(tys, id, &instances, &mut marked) {
            continue;
        }
        match crate::refs::identity(paired, at) {
            Ok(identity) => {
                let words = [u64::from(config.rate), u64::from(tys.ty(id).width)];
                slots.insert(id, crate::cache::mixed(identity, &words));
            }
            Err(refused) => {
                unslotted.get_or_insert_with(|| refused.to_string());
            }
        }
    }
    let typed = paired.lowered().to_vec();
    Ok(Volatile {
        slots,
        unslotted,
        typed,
    })
}

/// The target typed with each volatile parameter at a stand-in, and its root.
fn at_stand_in<'t>(
    graph: &Graph,
    target: &str,
    (config, stand_in): (&super::RenderConfig, &'t mut Typed),
) -> Result<(&'t Typing, NodeId), EngineError> {
    let names = &config.volatile;
    let held = |name: &str| {
        let at = names.iter().position(|n| n == name)?;
        Some(Expr::Lit(Literal::Num(STAND_IN + at as f64)))
    };
    let rebound = graph.rebound(&held);
    let inst = crate::instantiate::instantiate(&rebound, target, config.rate)?;
    let root = inst.instance_of(target)?;
    let order = crate::schedule::schedule_from(&inst, std::slice::from_ref(&root))?;
    let tys = stand_in.typed(&inst, &order)?;
    let at = tys.id(&root).ok_or(EngineError::UnknownNode(root))?;
    Ok((tys, at))
}

/// No knob lands on it by chance.
const STAND_IN: f64 = 0.618_033_988_749_894_8;

/// Each node beside the stand-in's node built the same way, walked down from the two roots
/// operand by operand.
fn pair((tys, root): (&Typing, NodeId), (stood, at): (&Typing, NodeId)) -> Vec<(NodeId, NodeId)> {
    let (mut out, mut seen, mut open) = (Vec::new(), BTreeSet::new(), vec![(root, at)]);
    while let Some((id, at)) = open.pop() {
        if !seen.insert(id) {
            continue;
        }
        out.push((id, at));
        let (mine, theirs) = (tys.operands(id), stood.operands(at));
        if mine.len() == theirs.len() && alike(tys, id, stood, at) {
            open.extend(mine.into_iter().zip(theirs));
        }
    }
    out
}

/// One construct, whatever numbers it holds.
fn alike(tys: &Typing, id: NodeId, stood: &Typing, at: NodeId) -> bool {
    match (tys.value(id), stood.value(at)) {
        (Value::Op { name, .. }, Value::Op { name: theirs, .. }) => name == theirs,
        (Value::Cast(cast, _), Value::Cast(theirs, _)) => cast == theirs,
        (Value::Filter { shape, .. }, Value::Filter { shape: theirs, .. }) => shape == theirs,
        (Value::Solver { params, .. }, Value::Solver { params: theirs, .. }) => {
            params.name() == theirs.name()
        }
        (mine, theirs) => std::mem::discriminant(mine) == std::mem::discriminant(theirs),
    }
}

fn refuse_unbound(inst: &Instances, names: &[String], target: &str) -> Result<(), EngineError> {
    let bound: BTreeSet<&str> = inst.bound_names().collect();
    let Some(missing) = names.iter().find(|n| !bound.contains(n.as_str())) else {
        return Ok(());
    };
    let held: Vec<&str> = bound.into_iter().collect();
    Err(EngineError::refused(Diagnostic {
        code: "render.volatile_unbound".to_string(),
        message: format!(
            "`{missing}` is declared volatile, and nothing `{target}` reaches binds it"
        ),
        location: Located::at(target, None),
        help: match held.is_empty() {
            true => "this target binds no parameter; render it without a volatile name".to_string(),
            false => format!("name a parameter this target binds: {}", held.join(", ")),
        },
    }))
}

/// A node met again on its own path reads as settled: a loop closes through `self`.
fn volatile(
    tys: &Typing,
    id: NodeId,
    instances: &BTreeSet<&str>,
    marked: &mut HashMap<NodeId, bool>,
) -> bool {
    if let Some(held) = marked.get(&id) {
        return *held;
    }
    marked.insert(id, false);
    let held = instances.contains(tys.name(id))
        || tys
            .operands(id)
            .into_iter()
            .any(|op| volatile(tys, op, instances, marked));
    marked.insert(id, held);
    held
}

struct Reach<'a> {
    inst: &'a Instances,
    names: &'a [String],
    bound: HashMap<(ScopeId, String), bool>,
}

impl Reach<'_> {
    fn declared(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    fn instance(&mut self, path: &str) -> bool {
        let Some((_, cx)) = self.inst.at(path) else {
            return false;
        };
        let names: Vec<String> = self.vars(cx.scope);
        names.iter().any(|name| self.bound(cx.scope, name))
    }

    fn vars(&self, scope: ScopeId) -> Vec<String> {
        self.inst
            .vars(scope)
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// A caller's scope is always built before the scope it binds into, so this descends.
    fn bound(&mut self, scope: ScopeId, name: &str) -> bool {
        if self.declared(name) {
            return true;
        }
        if let Some(held) = self.bound.get(&(scope, name.to_string())) {
            return *held;
        }
        let held = match self.inst.binds(scope, name) {
            Some(thunk) => self.vars(thunk.scope).iter().any(|read| {
                sva_ast::occurs_free(thunk.expr, read) && self.bound(thunk.scope, read)
            }),
            None => false,
        };
        self.bound.insert((scope, name.to_string()), held);
        held
    }
}
