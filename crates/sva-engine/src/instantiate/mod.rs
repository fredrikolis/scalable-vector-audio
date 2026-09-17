// Concern: holds one instance per argument tuple and resolves a name inside one | Non-concern: building the table (build.rs), writing a resolved walk out (resolved.rs) | IO: (path) -> a body, a Node

mod build;
mod resolved;

use std::collections::{BTreeMap, HashMap};

use sva_ast::{Arg, BinOp, ByteSpan, Expr, Literal};
use sva_formula::filter::Shape;
use sva_formula::note;

use crate::error::EngineError;
use crate::vocabulary::is_builtin;

pub use build::{from_roots, instantiate};

pub(crate) type ScopeId = u32;

/// Scope 0 binds nothing, reserved for defaults: a default stands outside every invocation.
pub(crate) const NO_PARAMS: ScopeId = 0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Thunk<'g> {
    pub(crate) expr: &'g Expr,
    pub(crate) scope: ScopeId,
}

/// Sorted by name: the tuple names the instance, not the call order.
#[derive(Debug)]
pub(crate) struct Scope<'g> {
    pub(crate) vars: Vec<(String, Thunk<'g>)>,
    keys: Vec<u64>,
    seen: u64,
}

/// A name's length and first seven bytes in one word, so a lookup costs integer compares.
#[inline]
pub(crate) fn packed(name: &str) -> u64 {
    let bytes = name.as_bytes();
    let mut key = (bytes.len() as u64) << 56;
    for (at, byte) in bytes.iter().take(7).enumerate() {
        key |= u64::from(*byte) << (at * 8);
    }
    key
}

#[inline]
fn bit(key: u64) -> u64 {
    1u64 << ((key ^ (key >> 32)) & 63)
}

impl<'g> Scope<'g> {
    pub(crate) fn new(vars: Vec<(String, Thunk<'g>)>) -> Scope<'g> {
        let keys: Vec<u64> = vars.iter().map(|(k, _)| packed(k)).collect();
        let seen = keys.iter().fold(0, |acc, k| acc | bit(*k));
        Scope { vars, keys, seen }
    }

    /// One bit says no before a key is read. Two names of a length share a key past the
    /// seventh byte, so every match is offered the name.
    #[inline]
    pub(crate) fn get(&self, name: &str, key: u64) -> Option<Thunk<'g>> {
        if self.seen & bit(key) == 0 {
            return None;
        }
        self.keys
            .iter()
            .enumerate()
            .find(|(at, k)| **k == key && (name.len() <= 7 || self.vars[*at].0 == name))
            .map(|(at, _)| self.vars[at].1)
    }
}

/// `time` is DYNAMIC: `p(t - d)` moves every `t` under the parameter, however deep.
#[derive(Clone, Copy)]
pub struct Cx<'a> {
    pub(crate) scope: ScopeId,
    pub(crate) time: Option<&'a Time<'a>>,
}

pub(crate) struct Time<'a> {
    pub(crate) expr: &'a Expr,
    pub(crate) cx: Cx<'a>,
}

enum Move<'a> {
    Here(&'a Expr, Cx<'a>),
    Shifted(&'a Expr, ScopeId, &'a Expr),
}

impl<'a> Cx<'a> {
    pub(crate) fn root(scope: ScopeId) -> Cx<'a> {
        Cx { scope, time: None }
    }

    pub(crate) fn under(self, scope: ScopeId) -> Cx<'a> {
        Cx {
            scope,
            time: self.time,
        }
    }
}

/// Read from sva-ast so the two crates cannot resolve a path two ways.
pub fn resolve_ref_path(referencing: &str, ref_path: &str) -> Result<String, EngineError> {
    sva_ast::resolve_ref_path(referencing, ref_path)
        .ok_or_else(|| EngineError::RefAboveRoot(referencing.to_string(), ref_path.to_string()))
}

/// `@kick.comb(delay=0.03)` is `@comb(t, x=@kick, delay=0.03)`: one name, so a file never
/// states a parameter order.
pub const SIGNAL_PARAM: &str = "x";

/// Bounds BREADTH: a chain of functions each invoking the next several times multiplies.
pub const MAX_INSTANCES: usize = 4096;

/// A `Read` names the child INSTANCE, so this is reachable only past [`Instances::follow`].
#[derive(Clone, Copy)]
pub enum Node<'a> {
    Lit(&'a Literal),
    Name(&'a str),
    Bin(BinOp, &'a Expr, &'a Expr),
    Call {
        name: &'a str,
        args: &'a [Arg],
        span: ByteSpan,
    },
    Read {
        path: &'a str,
        arg: &'a Expr,
        span: ByteSpan,
    },
    Own {
        arg: &'a Expr,
        span: ByteSpan,
    },
}

/// One file's body beside what its parameters stand for: the body is shared, the scope is not.
#[derive(Debug)]
pub struct Instances<'g> {
    pub(crate) scopes: Vec<Scope<'g>>,
    pub(crate) sites: HashMap<(usize, ScopeId), String>,
    pub(crate) nodes: BTreeMap<String, Thunk<'g>>,
    pub(crate) origin: BTreeMap<String, String>,
    pub(crate) own_terms: BTreeMap<String, String>,
    pub(crate) root: String,
    pub(crate) time: Expr,
}

impl<'g> Instances<'g> {
    pub fn origin(&self, instance: &str) -> Option<&str> {
        self.origin.get(instance).map(String::as_str)
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.nodes.keys().map(String::as_str)
    }

    pub fn holds(&self, path: &str) -> bool {
        self.nodes.contains_key(path)
    }

    /// A bare file name is the file read as a root, on its own terms; a call site's
    /// instance carries the bindings that made it.
    pub fn instance_of(&self, target: &str) -> Result<String, EngineError> {
        if self.holds(target) {
            return Ok(target.to_string());
        }
        if let Some(own) = self.own_terms.get(target) {
            return Ok(own.clone());
        }
        sole(
            target,
            self.instances_of(target).map(|p| (p.clone(), p)).collect(),
        )
    }

    pub fn instances_of<'a>(&'a self, file: &'a str) -> impl Iterator<Item = String> + 'a {
        self.paths()
            .filter(move |p| self.origin(p) == Some(file))
            .map(str::to_string)
    }

    pub fn at<'a>(&'a self, path: &str) -> Option<(&'a Expr, Cx<'a>)> {
        let thunk = *self.nodes.get(path)?;
        Some((thunk.expr, Cx::root(thunk.scope)))
    }

    pub fn bindings<'a>(&'a self, path: &str) -> Option<Vec<(&'a str, &'a Expr, Cx<'a>)>> {
        let thunk = *self.nodes.get(path)?;
        Some(
            self.scopes[thunk.scope as usize]
                .vars
                .iter()
                .map(|(name, value)| (name.as_str(), value.expr, Cx::root(value.scope)))
                .collect(),
        )
    }
}

/// The one node a written name stands for: a file one argument tuple expanded is that
/// instance, and a file several tuples share is none of them.
pub(crate) fn sole<T>(target: &str, mut held: Vec<(String, T)>) -> Result<T, EngineError> {
    match held.len() {
        0 => Err(EngineError::UnknownNode(target.to_string())),
        1 => Ok(held.remove(0).1),
        _ => Err(EngineError::AmbiguousNode(
            target.to_string(),
            held.into_iter().map(|(name, _)| name).collect(),
        )),
    }
}

/// A name the language answers, so no binding reaches it.
pub(crate) fn is_free_name(name: &str) -> bool {
    matches!(name, "t" | "f" | "i" | "pi" | "inf") || note::frequency(name).is_some()
}

pub fn is_reserved(name: &str) -> bool {
    is_free_name(name) || name == "self" || is_builtin(name)
}

impl<'g> Instances<'g> {
    #[inline]
    pub(crate) fn binds(&self, scope: ScopeId, name: &str) -> Option<Thunk<'g>> {
        self.scopes[scope as usize].get(name, packed(name))
    }

    /// A bound name continues in what it stands for; anything else is read where it stands.
    #[inline]
    pub fn follow<'a, R>(
        &'a self,
        e: &'a Expr,
        cx: Cx<'a>,
        go: impl FnOnce(&'a Expr, Cx<'_>) -> R,
    ) -> Option<R> {
        match self.step(e, cx)? {
            Move::Here(e2, cx2) => Some(go(e2, cx2)),
            Move::Shifted(e2, scope, when) => {
                let moved = Time { expr: when, cx };
                Some(go(
                    e2,
                    Cx {
                        scope,
                        time: Some(&moved),
                    },
                ))
            }
        }
    }

    /// A walk of two at once needs both shifts in one frame, so it cannot take the closure.
    #[inline(always)]
    fn step<'a>(&'a self, e: &'a Expr, cx: Cx<'a>) -> Option<Move<'a>> {
        match e {
            Expr::Var(name) if name == "t" => cx.time.map(|t| Move::Here(t.expr, t.cx)),
            Expr::Var(name) => self
                .binds(cx.scope, name)
                .map(|b| Move::Here(b.expr, cx.under(b.scope))),
            Expr::Call { name, args, .. } => {
                let bound = self.binds(cx.scope, name)?;
                let [Arg::Pos(when)] = args.as_slice() else {
                    return None;
                };
                Some(Move::Shifted(bound.expr, bound.scope, when))
            }
            _ => None,
        }
    }

    #[inline]
    pub fn node<'a>(&'a self, e: &'a Expr, cx: Cx<'_>) -> Node<'a> {
        match e {
            Expr::Lit(l) => Node::Lit(l),
            Expr::Var(name) => Node::Name(name),
            Expr::Bin(op, l, r) => Node::Bin(*op, l, r),
            Expr::SelfRef { arg, span } => Node::Own { arg, span: *span },
            Expr::Ref {
                path, arg, span, ..
            } => Node::Read {
                path: self.site(e, cx.scope, path),
                arg,
                span: *span,
            },
            Expr::Call { name, args, span } if is_builtin(name) => Node::Call {
                name,
                args,
                span: *span,
            },
            Expr::Call { name, span, .. } => Node::Read {
                path: self.site(e, cx.scope, name),
                arg: &self.time,
                span: *span,
            },
        }
    }

    fn site<'a>(&'a self, e: &Expr, scope: ScopeId, written: &'a str) -> &'a str {
        match self.sites.get(&(std::ptr::from_ref(e) as usize, scope)) {
            Some(child) => child,
            None => written,
        }
    }

    pub(crate) fn is_now(&self, e: &Expr, cx: Cx) -> bool {
        if let Some(r) = self.follow(e, cx, |e2, cx2| self.is_now(e2, cx2)) {
            return r;
        }
        matches!(self.node(e, cx), Node::Name(n) if n == "t")
    }

    pub fn reads_self(&self, path: &str) -> bool {
        self.at(path).is_some_and(|(e, cx)| self.holds_self(e, cx))
    }

    pub(crate) fn holds_self(&self, e: &Expr, cx: Cx) -> bool {
        if let Some(r) = self.follow(e, cx, |e2, cx2| self.holds_self(e2, cx2)) {
            return r;
        }
        match self.node(e, cx) {
            Node::Lit(_) | Node::Name(_) => false,
            Node::Own { .. } => true,
            Node::Bin(_, l, r) => self.holds_self(l, cx) || self.holds_self(r, cx),
            Node::Read { arg, .. } => self.holds_self(arg, cx),
            Node::Call { args, .. } => args.iter().any(|a| {
                let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                self.holds_self(x, cx)
            }),
        }
    }

    /// A value depending on WHICH sample is written rather than on the time handed to it.
    pub(crate) fn position_dependent(&self, e: &Expr, cx: Cx) -> Option<String> {
        if let Some(r) = self.follow(e, cx, |e2, cx2| self.position_dependent(e2, cx2)) {
            return r;
        }
        match self.node(e, cx) {
            Node::Lit(_) | Node::Name(_) => None,
            Node::Own { .. } => Some("self".to_string()),
            Node::Bin(_, l, r) => self
                .position_dependent(l, cx)
                .or_else(|| self.position_dependent(r, cx)),
            Node::Read { arg, .. } => self.position_dependent(arg, cx),
            Node::Call { name, args, .. } => {
                if Shape::from_name(name).is_some() || name == "crop" {
                    return Some(name.to_string());
                }
                args.iter().find_map(|a| {
                    let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                    self.position_dependent(x, cx)
                })
            }
        }
    }

    /// Two arguments are one tuple when they MEAN the same, walked both at once.
    pub(crate) fn same<'x, 'y>(&self, a: &Expr, ax: Cx<'x>, b: &Expr, by: Cx<'y>) -> bool {
        if let Some(moved) = self.step(a, ax) {
            return match moved {
                Move::Here(a2, ax2) => self.same(a2, ax2, b, by),
                Move::Shifted(a2, scope, when) => {
                    let moved = Time { expr: when, cx: ax };
                    self.same(
                        a2,
                        Cx {
                            scope,
                            time: Some(&moved),
                        },
                        b,
                        by,
                    )
                }
            };
        }
        if let Some(moved) = self.step(b, by) {
            return match moved {
                Move::Here(b2, by2) => self.same(a, ax, b2, by2),
                Move::Shifted(b2, scope, when) => {
                    let moved = Time { expr: when, cx: by };
                    self.same(
                        a,
                        ax,
                        b2,
                        Cx {
                            scope,
                            time: Some(&moved),
                        },
                    )
                }
            };
        }
        match (self.node(a, ax), self.node(b, by)) {
            (Node::Lit(x), Node::Lit(y)) => x == y,
            (Node::Name(x), Node::Name(y)) => x == y,
            (Node::Bin(o1, l1, r1), Node::Bin(o2, l2, r2)) => {
                o1 == o2 && self.same(l1, ax, l2, by) && self.same(r1, ax, r2, by)
            }
            (Node::Own { arg: x, .. }, Node::Own { arg: y, .. }) => self.same(x, ax, y, by),
            (
                Node::Read {
                    path: p, arg: x, ..
                },
                Node::Read {
                    path: q, arg: y, ..
                },
            ) => p == q && self.same(x, ax, y, by),
            (
                Node::Call {
                    name: n1, args: a1, ..
                },
                Node::Call {
                    name: n2, args: a2, ..
                },
            ) => {
                n1 == n2
                    && a1.len() == a2.len()
                    && a1.iter().zip(a2).all(|(x, y)| match (x, y) {
                        (Arg::Pos(x), Arg::Pos(y)) => self.same(x, ax, y, by),
                        (Arg::Named(k1, x), Arg::Named(k2, y)) => {
                            k1 == k2 && self.same(x, ax, y, by)
                        }
                        _ => false,
                    })
            }
            _ => false,
        }
    }

    pub(crate) fn same_binds(&self, scope: ScopeId, binds: &[(String, Thunk<'g>)]) -> bool {
        let held = &self.scopes[scope as usize].vars;
        held.len() == binds.len()
            && held.iter().zip(binds).all(|((k1, v1), (k2, v2))| {
                k1 == k2 && self.same(v1.expr, Cx::root(v1.scope), v2.expr, Cx::root(v2.scope))
            })
    }
}
