// Concern: holds one instance per argument tuple and resolves a name inside one | Non-concern: building the table (build.rs), writing a resolved walk out (resolved.rs) | IO: (path) -> a body, a Node

mod build;
mod edges;
mod resolved;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use sva_ast::{Address, Arg, BinOp, ByteSpan, Defined, Expr, Literal};

use crate::error::EngineError;
use crate::time::Grid;
use sva_ast::is_builtin;

use build::Journal;
pub use build::{from_roots, instantiate};
use edges::Edges;
pub(crate) use resolved::Resolution;

pub(crate) type ScopeId = u32;

/// Scope 0 binds nothing, reserved for defaults: a default stands outside every invocation.
pub(crate) const NO_PARAMS: ScopeId = 0;

#[derive(Clone, Copy, Debug)]
pub struct Thunk<'a> {
    pub(crate) expr: &'a Expr,
    pub(crate) scope: ScopeId,
}

/// An expression owned where it was written: a node's line and each child taken down to it.
#[derive(Clone, Debug)]
pub(crate) struct Written {
    defined: Arc<Defined>,
    path: Arc<[u32]>,
}

impl Written {
    /// `line` 0 is the body, `k` the default line `k - 1`.
    pub(crate) fn of(defined: &Arc<Defined>, line: u32) -> Written {
        Written {
            defined: Arc::clone(defined),
            path: [line].into(),
        }
    }

    pub(crate) fn expr(&self) -> &Expr {
        let (line, path) = self.path.split_first().expect("a line");
        let top = match *line {
            0 => &self.defined.body,
            k => &self.defined.defaults[k as usize - 1].1,
        };
        let mut at = top;
        for k in path {
            at = child(at, *k).expect("a path down this expression");
        }
        at
    }

    /// Whether this is an argument a call was given by position, not by name.
    pub(crate) fn is_positional(&self) -> bool {
        let Some((last, upper)) = self.path[1..].split_last() else {
            return false;
        };
        let parent = Written {
            defined: Arc::clone(&self.defined),
            path: [&self.path[..1], upper].concat().into(),
        };
        match parent.expr() {
            Expr::Call { args, .. } => matches!(args.get(*last as usize), Some(Arg::Pos(_))),
            _ => false,
        }
    }
}

/// A child's number on a walk down, as [`child`] reads it.
#[derive(Clone, Copy)]
pub(crate) enum Child {
    First,
    Right,
    Arg(u32),
    Bind(u32),
}

impl Child {
    pub(crate) fn index(self) -> u32 {
        match self {
            Child::First => 0,
            Child::Right => 1,
            Child::Arg(k) => k,
            Child::Bind(k) => k + 1,
        }
    }
}

fn child(e: &Expr, k: u32) -> Option<&Expr> {
    let k = k as usize;
    match e {
        Expr::Bin(_, l, r) => [l, r].get(k).map(|c| &***c),
        Expr::Call { args, .. } => args.get(k).map(|(Arg::Pos(x) | Arg::Named(_, x))| x),
        Expr::Ref { arg, binds, .. } => match k {
            0 => Some(arg),
            k => binds.get(k - 1).map(|(_, x)| x),
        },
        Expr::SelfRef { arg, .. } | Expr::Indexed { arg, .. } => (k == 0).then_some(&**arg),
        Expr::Lit(_) | Expr::Var(_) => None,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Bound {
    pub(crate) written: Written,
    pub(crate) scope: ScopeId,
}

impl Bound {
    pub(crate) fn thunk(&self) -> Thunk<'_> {
        Thunk {
            expr: self.written.expr(),
            scope: self.scope,
        }
    }
}

/// Sorted by name: the tuple names the instance. Held while owned or read in.
#[derive(Debug)]
pub(crate) struct Scope {
    pub(crate) vars: Vec<(String, Bound)>,
    keys: Vec<u64>,
    seen: u64,
    chained: bool,
    held: u32,
    /// The child instance each invocation scanned in it names, by where it is written.
    sites: HashMap<usize, String>,
}

/// One instance: what it reads, who reads it, where it was first invoked.
#[derive(Debug)]
pub(crate) struct Instance {
    pub(crate) body: Bound,
    pub(crate) file: String,
    given: Vec<String>,
    site: (String, Option<ByteSpan>),
    held: u32,
    scanned: bool,
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

impl Scope {
    pub(crate) fn new(vars: Vec<(String, Bound)>, chained: bool) -> Scope {
        let keys: Vec<u64> = vars.iter().map(|(k, _)| packed(k)).collect();
        let seen = keys.iter().fold(0, |acc, k| acc | bit(*k));
        Scope {
            vars,
            keys,
            seen,
            chained,
            held: 0,
            sites: HashMap::new(),
        }
    }

    /// One bit says no before a key is read. Two names of a length share a key past the
    /// seventh byte, so every match is offered the name.
    #[inline]
    pub(crate) fn get(&self, name: &str, key: u64) -> Option<&Bound> {
        if self.seen & bit(key) == 0 {
            return None;
        }
        self.keys
            .iter()
            .enumerate()
            .find(|(at, k)| **k == key && (name.len() <= 7 || self.vars[*at].0 == name))
            .map(|(at, _)| &self.vars[at].1)
    }
}

/// `time` is DYNAMIC: `p(t - d)` moves every `t` under the parameter, however deep. `sp` is a
/// step of `grid`.
#[derive(Clone, Copy)]
pub struct Cx<'a> {
    pub(crate) scope: ScopeId,
    pub(crate) time: Option<&'a Time<'a>>,
    pub(crate) grid: Grid,
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
    pub(crate) fn under(self, scope: ScopeId) -> Cx<'a> {
        Cx { scope, ..self }
    }

    pub(crate) fn on(self, grid: Grid) -> Cx<'a> {
        Cx { grid, ..self }
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
        address: Address,
        span: ByteSpan,
    },
    Own {
        arg: &'a Expr,
        address: Address,
        span: ByteSpan,
    },
    /// `x[i]`: `of`, what `x` is bound to, read at index `arg`.
    Signal {
        name: &'a str,
        of: Thunk<'a>,
        arg: &'a Expr,
        span: ByteSpan,
    },
}

/// One file's body beside what its parameters stand for: the body is shared, the scope is not.
/// An instance lasts while a root or a reader holds it.
#[derive(Debug)]
pub struct Instances {
    pub(crate) scopes: Vec<Option<Scope>>,
    free: Vec<ScopeId>,
    nodes: BTreeMap<String, Instance>,
    edges: Edges,
    pub(crate) own_terms: BTreeMap<String, String>,
    files: BTreeMap<String, BTreeSet<String>>,
    journal: Journal,
    pub(crate) time: Expr,
    /// The rate in use, whose step `sp` is.
    pub(crate) rate: u32,
}

impl Instances {
    pub fn new(rate: u32) -> Instances {
        Instances {
            scopes: vec![Some(Scope::new(Vec::new(), false))],
            free: Vec::new(),
            nodes: BTreeMap::new(),
            edges: Edges::default(),
            own_terms: BTreeMap::new(),
            files: BTreeMap::new(),
            journal: Journal::default(),
            time: Expr::Var("t".to_string()),
            rate,
        }
    }

    pub(crate) fn rate(&self) -> u32 {
        self.rate
    }

    pub(crate) fn grid(&self) -> Grid {
        Grid::of(self.rate)
    }

    pub(crate) fn cx(&self, scope: ScopeId) -> Cx<'_> {
        Cx {
            scope,
            time: None,
            grid: self.grid(),
        }
    }

    pub fn origin(&self, instance: &str) -> Option<&str> {
        self.nodes.get(instance).map(|held| held.file.as_str())
    }

    pub fn deps(&self, path: &str) -> &[String] {
        self.edges.of(path)
    }

    pub(crate) fn readers(&self, path: &str) -> impl Iterator<Item = &str> {
        self.edges.readers(path)
    }

    /// `name` held as `held`, filed under its file: the one way an instance is added.
    fn insert(&mut self, name: String, held: Instance) {
        let files = self.files.entry(held.file.clone()).or_default();
        files.insert(name.clone());
        self.nodes.insert(name, held);
    }

    /// `name` let go, reading nothing and filed nowhere: the one way an instance goes.
    fn remove(&mut self, name: &str) -> (Instance, Vec<String>) {
        let held = self.nodes.remove(name).expect("an instance held");
        let files = self
            .files
            .get_mut(&held.file)
            .expect("its file's instances");
        files.remove(name);
        if files.is_empty() {
            self.files.remove(&held.file);
        }
        (held, self.edges.cleared(name))
    }

    pub(crate) fn scope(&self, id: ScopeId) -> &Scope {
        self.scopes[id as usize]
            .as_ref()
            .expect("a scope something holds")
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
        let held = self.files.get(file).into_iter().flatten();
        held.map(String::clone)
    }

    pub fn at<'a>(&'a self, path: &str) -> Option<(&'a Expr, Cx<'a>)> {
        let thunk = self.nodes.get(path)?.body.thunk();
        Some((thunk.expr, self.cx(thunk.scope)))
    }

    pub fn bindings<'a>(&'a self, path: &str) -> Option<Vec<(&'a str, &'a Expr, Cx<'a>)>> {
        let scope = self.nodes.get(path)?.body.scope;
        Some(
            self.vars(scope)
                .map(|(name, value)| (name, value.expr, self.cx(value.scope)))
                .collect(),
        )
    }

    /// Each name `scope` binds, and what it stands for.
    pub(crate) fn vars(&self, scope: ScopeId) -> impl Iterator<Item = (&str, Thunk<'_>)> {
        let vars = self.scope(scope).vars.iter();
        vars.map(|(name, bound)| (name.as_str(), bound.thunk()))
    }

    /// Every name any scope binds.
    pub(crate) fn bound_names(&self) -> impl Iterator<Item = &str> {
        let vars = self
            .scopes
            .iter()
            .flatten()
            .flat_map(|scope| scope.vars.iter());
        vars.map(|(name, _)| name.as_str())
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

fn implicit_time(name: &str) -> bool {
    sva_ast::FINITE_DIFFERENCE.contains(&name)
        || sva_ast::MODAL.contains(&name)
        || matches!(name, "noise" | "stft" | "istft")
}

impl Instances {
    #[inline]
    pub(crate) fn binds(&self, scope: ScopeId, name: &str) -> Option<Thunk<'_>> {
        let bound = self.scope(scope).get(name, packed(name))?;
        Some(bound.thunk())
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
                        grid: cx.grid,
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
            Expr::SelfRef { arg, address, span } => Node::Own {
                arg,
                address: *address,
                span: *span,
            },
            Expr::Ref {
                path,
                arg,
                address,
                span,
                ..
            } => Node::Read {
                path: self.site(e, cx.scope, path),
                arg,
                address: *address,
                span: *span,
            },
            Expr::Indexed { name, arg, span } => match self.binds(cx.scope, name) {
                Some(of) => Node::Signal {
                    name,
                    of,
                    arg,
                    span: *span,
                },
                None => Node::Name(name),
            },
            Expr::Call { name, args, span } if is_builtin(name) => Node::Call {
                name,
                args,
                span: *span,
            },
            Expr::Call { name, span, .. } => Node::Read {
                path: self.site(e, cx.scope, name),
                arg: &self.time,
                address: Address::Time,
                span: *span,
            },
        }
    }

    fn site<'a>(&'a self, e: &Expr, scope: ScopeId, written: &'a str) -> &'a str {
        match self
            .scope(scope)
            .sites
            .get(&(std::ptr::from_ref(e) as usize))
        {
            Some(child) => child,
            None => written,
        }
    }

    /// A parameter's signal at the reader's own `t`: no shift the reader is under moves it.
    pub(crate) fn signal<'a>(&self, of: Thunk<'a>, cx: Cx<'a>) -> Cx<'a> {
        Cx {
            scope: of.scope,
            time: None,
            grid: cx.grid,
        }
    }

    pub(crate) fn is_now(&self, e: &Expr, cx: Cx) -> bool {
        if let Some(r) = self.follow(e, cx, |e2, cx2| self.is_now(e2, cx2)) {
            return r;
        }
        matches!(self.node(e, cx), Node::Name(n) if n == "t")
    }

    /// One hop down; `self(...)` is no edge.
    fn direct_deps(&self, path: &str) -> Vec<String> {
        let (e, cx) = self.at(path).expect("an instance");
        let mut out = Vec::new();
        self.direct_refs(e, cx, &mut out);
        out.sort();
        out.dedup();
        out
    }

    fn direct_refs(&self, e: &Expr, cx: Cx, out: &mut Vec<String>) {
        if self
            .follow(e, cx, |e2, cx2| self.direct_refs(e2, cx2, out))
            .is_some()
        {
            return;
        }
        match self.node(e, cx) {
            Node::Lit(_) | Node::Name(_) => {}
            Node::Bin(_, l, r) => {
                self.direct_refs(l, cx, out);
                self.direct_refs(r, cx, out);
            }
            Node::Call { args, .. } => {
                for a in args {
                    let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                    self.direct_refs(x, cx, out);
                }
            }
            Node::Read { path, arg, .. } => {
                out.push(path.to_string());
                self.direct_refs(arg, cx, out);
            }
            Node::Own { arg, .. } => self.direct_refs(arg, cx, out),
            Node::Signal { of, arg, .. } => {
                self.direct_refs(of.expr, self.signal(of, cx), out);
                self.direct_refs(arg, cx, out);
            }
        }
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
            Node::Read { arg, .. } | Node::Signal { arg, .. } => self.holds_self(arg, cx),
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
            Node::Read { arg, .. } | Node::Signal { arg, .. } => self.position_dependent(arg, cx),
            Node::Call { name, args, .. } => {
                if crate::vocabulary::shape(name).is_some() || name == "crop" || implicit_time(name)
                {
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
                            grid: ax.grid,
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
                            grid: by.grid,
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
            (
                Node::Own {
                    arg: x, address: i, ..
                },
                Node::Own {
                    arg: y, address: j, ..
                },
            ) => i == j && self.same(x, ax, y, by),
            (
                Node::Read {
                    path: p,
                    arg: x,
                    address: i,
                    ..
                },
                Node::Read {
                    path: q,
                    arg: y,
                    address: j,
                    ..
                },
            ) => p == q && i == j && self.same(x, ax, y, by),
            (Node::Signal { of: f, arg: x, .. }, Node::Signal { of: g, arg: y, .. }) => {
                self.same(f.expr, self.signal(f, ax), g.expr, self.signal(g, by))
                    && self.same(x, ax, y, by)
            }
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

    pub(crate) fn same_binds(&self, scope: ScopeId, binds: &[(String, Bound)]) -> bool {
        let held = &self.scope(scope).vars;
        held.len() == binds.len()
            && held.iter().zip(binds).all(|((k1, v1), (k2, v2))| {
                let (v1, v2) = (v1.thunk(), v2.thunk());
                k1 == k2 && self.same(v1.expr, self.cx(v1.scope), v2.expr, self.cx(v2.scope))
            })
    }
}
