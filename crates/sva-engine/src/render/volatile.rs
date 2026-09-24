// Concern: marks the nodes volatile parameters reach and names each one's slot | Non-concern: the slot store, what a node computes | IO: (Instances, Typing, names) -> a slot per volatile node

use std::collections::{BTreeMap, BTreeSet, HashMap};

use sva_ast::{Arg, Expr};
use sva_formula::{Hash, NodeId};

use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::{Cx, Instances, Node, ScopeId};
use crate::render::RenderConfig;
use crate::typing::{Typing, Value};

/// The base of each volatile node's slot; a node absent here reads and writes the stores.
#[derive(Default)]
pub(super) struct Volatile {
    slots: BTreeMap<NodeId, Hash>,
}

impl Volatile {
    pub(super) fn slot(&self, id: NodeId) -> Option<Hash> {
        self.slots.get(&id).copied()
    }
}

/// A node is volatile when its instance reads a volatile parameter, directly or through what a
/// caller bound, or when anything it is built from is.
pub(super) fn mark(
    inst: &Instances,
    tys: &Typing,
    config: &RenderConfig,
    target: &str,
) -> Result<Volatile, EngineError> {
    if config.volatile.is_empty() {
        return Ok(Volatile::default());
    }
    refuse_unbound(inst, &config.volatile, target)?;
    let mut reach = Reach {
        inst,
        names: &config.volatile,
        bound: HashMap::new(),
        text: HashMap::new(),
    };
    let instances: BTreeSet<&str> = inst.paths().filter(|p| reach.instance(p)).collect();
    let mut marked: Vec<Option<bool>> = vec![None; tys.len()];
    let mut ordinal: HashMap<&str, u64> = HashMap::new();
    let mut slots = BTreeMap::new();
    for id in (0..tys.len()).map(|at| NodeId(at as u32)) {
        let name = tys.name(id);
        let nth = ordinal.entry(name).or_default();
        *nth += 1;
        if !volatile(tys, id, &instances, &mut marked) {
            continue;
        }
        let mut sink = Sink::default();
        sink.text(&reach.stripped(name));
        for word in [
            *nth,
            u64::from(config.rate),
            config.horizon.start_secs.to_bits(),
            config.horizon.len(config.rate).unwrap_or(0) as u64,
            u64::from(tys.ty(id).width),
        ] {
            sink.0.word(word);
        }
        slots.insert(id, sink.0.finish());
    }
    Ok(Volatile { slots })
}

fn refuse_unbound(inst: &Instances, names: &[String], target: &str) -> Result<(), EngineError> {
    let bound: BTreeSet<&str> = inst
        .scopes
        .iter()
        .flat_map(|scope| scope.vars.iter().map(|(name, _)| name.as_str()))
        .collect();
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

/// Dependencies first; a node met again on its own path reads as settled, since a loop
/// closes through `self`, never through an edge here.
fn volatile(
    tys: &Typing,
    id: NodeId,
    instances: &BTreeSet<&str>,
    marked: &mut [Option<bool>],
) -> bool {
    if let Some(held) = marked[id.0 as usize] {
        return held;
    }
    marked[id.0 as usize] = Some(false);
    let held = instances.contains(tys.name(id))
        || operands(tys, id)
            .into_iter()
            .any(|op| volatile(tys, op, instances, marked));
    marked[id.0 as usize] = Some(held);
    held
}

/// The edges a node's identity is hashed over.
fn operands(tys: &Typing, id: NodeId) -> Vec<NodeId> {
    match tys.value(id) {
        Value::ClosedForm(form) => crate::refs::nodes_in(&form.body),
        Value::Cast(_, source) | Value::Read { source, .. } => vec![*source],
        Value::Op { args, .. } => args.clone(),
        Value::Filter {
            x, cutoff, q, gain, ..
        } => vec![*x, *cutoff, *q, *gain],
        Value::SelfAt(_) | Value::Grid(_) | Value::Solver(_) => Vec::new(),
    }
}

struct Reach<'a, 'g> {
    inst: &'a Instances<'g>,
    names: &'a [String],
    bound: HashMap<(ScopeId, String), bool>,
    text: HashMap<String, String>,
}

impl Reach<'_, '_> {
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
        self.inst.scopes[scope as usize]
            .vars
            .iter()
            .map(|(name, _)| name.clone())
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

    /// An instance's name with every volatile value written as its parameter's name, and every
    /// instance it reads named the same way, so a knob's values share one slot.
    fn stripped(&mut self, path: &str) -> String {
        if let Some(held) = self.text.get(path) {
            return held.clone();
        }
        self.text.insert(path.to_string(), path.to_string());
        let Some((_, cx)) = self.inst.at(path) else {
            return path.to_string();
        };
        let file = self.inst.origin(path).unwrap_or(path).to_string();
        let inst = self.inst;
        let mut args = Vec::new();
        for (name, thunk) in &inst.scopes[cx.scope as usize].vars {
            let value = match self.declared(name) {
                true => format!("?{name}"),
                false => sva_ast::render_expr(&self.copy(thunk.expr, Cx::root(thunk.scope))),
            };
            args.push(format!("{name}={value}"));
        }
        let text = format!("{file}({})", args.join(", "));
        self.text.insert(path.to_string(), text.clone());
        text
    }

    fn copy(&mut self, e: &Expr, cx: Cx) -> Expr {
        let inst = self.inst;
        if let Expr::Var(name) | Expr::Call { name, .. } = e
            && self.declared(name)
            && inst.binds(cx.scope, name).is_some()
        {
            return Expr::Var(format!("?{name}"));
        }
        if let Some(r) = inst.follow(e, cx, |e2, cx2| self.copy(e2, cx2)) {
            return r;
        }
        match inst.node(e, cx) {
            Node::Lit(l) => Expr::Lit(l.clone()),
            Node::Name(name) => Expr::Var(name.to_string()),
            Node::Bin(op, l, r) => {
                Expr::Bin(op, Box::new(self.copy(l, cx)), Box::new(self.copy(r, cx)))
            }
            Node::Call { name, args, span } => Expr::Call {
                name: name.to_string(),
                args: args
                    .iter()
                    .map(|a| match a {
                        Arg::Pos(x) => Arg::Pos(self.copy(x, cx)),
                        Arg::Named(k, x) => Arg::Named(k.clone(), self.copy(x, cx)),
                    })
                    .collect(),
                span,
            },
            Node::Read { path, arg, span } => Expr::Ref {
                path: match inst.holds(path) {
                    true => self.stripped(path),
                    false => path.to_string(),
                },
                arg: Box::new(self.copy(arg, cx)),
                binds: Vec::new(),
                span,
            },
            Node::Own { arg, span } => Expr::SelfRef {
                arg: Box::new(self.copy(arg, cx)),
                span,
            },
        }
    }
}

const SLOT_ROTATE: u32 = 19;

#[derive(Default)]
struct Sink(sva_formula::Lanes<SLOT_ROTATE>);

impl Sink {
    fn text(&mut self, what: &str) {
        self.0.word(what.len() as u64);
        for byte in what.as_bytes() {
            self.0.word(u64::from(*byte));
        }
    }
}
