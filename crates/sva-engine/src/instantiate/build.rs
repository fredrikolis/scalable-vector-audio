// Concern: names one instance per argument tuple, scans each once, lets go of what nothing reads | Non-concern: what an instance holds (mod.rs) | IO: (&Graph, roots or rewritten nodes) -> Instances

use std::collections::BTreeSet;
use std::sync::Arc;

use sva_ast::{Arg, ByteSpan, Defined, Expr, Graph};

use crate::error::{BindingFault, EngineError};
use crate::instantiate::{
    Bound, Child, Instance, Instances, MAX_INSTANCES, NO_PARAMS, SIGNAL_PARAM, Scope, ScopeId,
    Written, resolve_ref_path,
};
use sva_ast::{SERIES, is_builtin, is_language_value, is_reserved};

pub fn instantiate(graph: &Graph, root: &str, rate: u32) -> Result<Instances, EngineError> {
    Ok(from_roots(graph, &[root.to_string()], rate)?.0)
}

/// One table over several roots, so a node two roots reach is one instance and one buffer.
pub fn from_roots(
    graph: &Graph,
    roots: &[String],
    rate: u32,
) -> Result<(Instances, Vec<String>), EngineError> {
    let mut out = Instances::new(rate);
    let named = out.hold(graph, roots)?;
    out.commit();
    Ok((out, named))
}

/// What a change named and scanned again since the table last settled, which `abort` undoes:
/// each instance named, each scanned again beside its old body and reads, and each read it
/// let go of.
#[derive(Debug, Default)]
pub(crate) struct Journal {
    named: Vec<String>,
    rescanned: Vec<(String, Bound, Vec<String>)>,
    lost: Vec<String>,
    held: Vec<(String, String)>,
}

impl Instances {
    /// Holds each of `roots` as a root, naming and scanning all they reach, until `commit` or
    /// `abort`; on a refusal the table is as it was.
    pub(crate) fn hold(
        &mut self,
        graph: &Graph,
        roots: &[String],
    ) -> Result<Vec<String>, EngineError> {
        let mut b = Builder::new(graph, self);
        let mut named = Vec::with_capacity(roots.len());
        for root in roots {
            match b.intern(root, Vec::new(), (root, None)) {
                Ok(name) => named.push(name),
                Err(e) => {
                    self.abort();
                    return Err(e);
                }
            }
        }
        b.work = named.iter().rev().cloned().collect();
        b.scanned_all()?;
        for (root, name) in roots.iter().zip(&named) {
            self.own_terms.insert(root.clone(), name.clone());
            self.nodes.get_mut(name).expect("a root named").held += 1;
            self.journal.held.push((root.clone(), name.clone()));
        }
        Ok(named)
    }

    /// Scans again each instance of each of `files`, each a node with no parameter that the
    /// graph now defines otherwise, until `commit` or `abort`; on a refusal the table is as it
    /// was.
    pub(crate) fn rewrite(&mut self, graph: &Graph, files: &[String]) -> Result<(), EngineError> {
        let mut b = Builder::new(graph, self);
        for file in files {
            let defined = graph.defined(file).expect("a node the graph defines");
            for name in b.out.instances_of(file).collect::<Vec<_>>() {
                let old = b.out.nodes[&name].body.clone();
                let deps = b.out.deps(&name).to_vec();
                let vars = b.out.scope(old.scope).vars.clone();
                debug_assert!(vars.is_empty(), "a rewritten node binds nothing");
                let scope = b.out.made(vars, false);
                b.out.retain(scope);
                let held = b.out.nodes.get_mut(&name).expect("an instance of the file");
                held.body = Bound {
                    written: Written::of(defined, 0),
                    scope,
                };
                held.scanned = false;
                b.out.journal.rescanned.push((name.clone(), old, deps));
                b.work.push(name);
            }
        }
        b.scanned_all()
    }

    /// What the latest change named and scanned again, so far.
    pub(crate) fn changing(&self) -> (&[String], impl Iterator<Item = (&str, &[String])>) {
        let rescanned = self.journal.rescanned.iter();
        let rescanned = rescanned.map(|(name, _, before)| (name.as_str(), before.as_slice()));
        (&self.journal.named, rescanned)
    }

    /// The latest change held: what nothing reads any more is let go, and each old body's
    /// scope with it; those let go.
    pub(crate) fn commit(&mut self) -> Vec<String> {
        let journal = std::mem::take(&mut self.journal);
        for (_, old, _) in journal.rescanned {
            self.release_scope(old.scope);
        }
        let mut open = journal.lost;
        open.extend(journal.named);
        self.collect(open)
    }

    /// The table as it was before the latest change.
    pub(crate) fn abort(&mut self) {
        let journal = std::mem::take(&mut self.journal);
        for (root, name) in journal.held {
            let held = self.nodes.get_mut(&name).expect("a root held");
            held.held -= 1;
            if held.held == 0 {
                self.own_terms.remove(&root);
            }
        }
        for (name, old, before) in journal.rescanned.into_iter().rev() {
            self.edges.set(&name, before);
            let held = self.nodes.get_mut(&name).expect("a rescanned instance");
            let new = std::mem::replace(&mut held.body, old).scope;
            held.scanned = true;
            self.release_scope(new);
        }
        for name in journal.named.into_iter().rev() {
            let (held, _) = self.remove(&name);
            self.release_scope(held.body.scope);
        }
    }

    /// Lets go of each of `open` nothing holds, and of what only it held; one named twice goes
    /// once.
    fn collect(&mut self, mut open: Vec<String>) -> Vec<String> {
        let mut removed = Vec::new();
        while let Some(name) = open.pop() {
            let held = self.nodes.get(&name).map(|held| held.held);
            if held != Some(0) || !self.edges.unread(&name) {
                continue;
            }
            let (held, deps) = self.remove(&name);
            self.release_scope(held.body.scope);
            open.extend(deps);
            removed.push(name);
        }
        removed
    }

    /// A scope of `vars`, holding each scope one of them reads in; held by nothing yet.
    fn made(&mut self, vars: Vec<(String, Bound)>, chained: bool) -> ScopeId {
        for (_, bound) in &vars {
            self.retain(bound.scope);
        }
        let scope = Some(Scope::new(vars, chained));
        match self.free.pop() {
            Some(id) => {
                self.scopes[id as usize] = scope;
                id
            }
            None => {
                self.scopes.push(scope);
                (self.scopes.len() - 1) as ScopeId
            }
        }
    }

    fn retain(&mut self, id: ScopeId) {
        if id != NO_PARAMS {
            self.scopes[id as usize]
                .as_mut()
                .expect("a live scope")
                .held += 1;
        }
    }

    fn release_scope(&mut self, id: ScopeId) {
        if id != NO_PARAMS {
            let scope = self.scopes[id as usize].as_mut().expect("a live scope");
            scope.held -= 1;
            self.drop_unheld(id);
        }
    }

    /// Frees `id` where nothing holds it, and each scope its bindings read in is
    /// held once less.
    fn drop_unheld(&mut self, id: ScopeId) {
        let mut open = vec![id];
        while let Some(id) = open.pop() {
            let unheld = self.scopes[id as usize]
                .as_ref()
                .is_some_and(|s| s.held == 0);
            if id == NO_PARAMS || !unheld {
                continue;
            }
            let scope = self.scopes[id as usize].take().expect("an unheld scope");
            for (_, bound) in scope.vars {
                if bound.scope != NO_PARAMS {
                    let read = self.scopes[bound.scope as usize].as_mut();
                    read.expect("a scope a binding reads in").held -= 1;
                    open.push(bound.scope);
                }
            }
            self.free.push(id);
        }
    }
}

/// An unbound variable is the CALLER's omission, so it is reported where the invocation is
/// written; every other fault already knows its own site.
fn relocate(e: EngineError, caller: &str, at: Option<ByteSpan>) -> EngineError {
    match e {
        EngineError::Binding {
            fault: fault @ BindingFault::Unbound(..),
            ..
        } => EngineError::Binding {
            node: caller.to_string(),
            span: at,
            fault,
        },
        other => other,
    }
}

fn fault(node: &str, span: Option<ByteSpan>, fault: BindingFault) -> EngineError {
    EngineError::Binding {
        node: node.to_string(),
        span,
        fault,
    }
}

/// Whether an expression names any of the parameters bound so far.
fn names_any(e: &Expr, binds: &[(String, Bound)]) -> bool {
    binds.iter().any(|(name, _)| sva_ast::occurs_free(e, name))
}

/// The parameters a scan finds used, and the child instances it finds invoked.
type Sinks<'s> = (&'s mut BTreeSet<String>, &'s mut Vec<String>);

/// Where a scan stands in what it walks: the node it was written in, and the path down.
struct At {
    defined: Arc<Defined>,
    path: Vec<u32>,
}

impl At {
    fn of(written: &Written) -> At {
        At {
            defined: Arc::clone(&written.defined),
            path: written.path.to_vec(),
        }
    }

    fn here(&self, child: Child) -> Written {
        let mut path = self.path.clone();
        path.push(child.index());
        Written {
            defined: Arc::clone(&self.defined),
            path: path.into(),
        }
    }
}

/// Where a scan stands. `'a` is `file`'s short per-call borrow.
#[derive(Clone, Copy)]
struct In<'a> {
    file: &'a str,
    scope: ScopeId,
    at: Option<ByteSpan>,
}

impl<'a> In<'a> {
    fn spanned(self, span: ByteSpan) -> In<'a> {
        In {
            at: Some(span),
            ..self
        }
    }
}

/// One change to the table, journaled in it: what it names and scans again.
struct Builder<'b> {
    graph: &'b Graph,
    out: &'b mut Instances,
    /// The series indices in scope, innermost last.
    indices: Vec<String>,
    work: Vec<String>,
}

impl<'b> Builder<'b> {
    fn new(graph: &'b Graph, out: &'b mut Instances) -> Builder<'b> {
        Builder {
            graph,
            out,
            indices: Vec::new(),
            work: Vec::new(),
        }
    }

    /// Scans all left to scan, then joins each scanned instance to what it reads; on a
    /// refusal the latest change is undone.
    fn scanned_all(mut self) -> Result<(), EngineError> {
        while let Some(name) = self.work.pop() {
            let held = self.out.nodes.get_mut(&name).expect("an instance to scan");
            if std::mem::replace(&mut held.scanned, true) {
                continue;
            }
            if let Err(e) = self.scanned(&name) {
                self.out.abort();
                return Err(e);
            }
        }
        for name in self.out.journal.named.clone() {
            let deps = self.out.direct_deps(&name);
            self.out.edges.set(&name, deps);
        }
        let rescanned = self.out.journal.rescanned.iter();
        let rescanned: Vec<(String, Vec<String>)> = rescanned
            .map(|(name, _, before)| (name.clone(), before.clone()))
            .collect();
        for (name, before) in rescanned {
            let deps = self.out.direct_deps(&name);
            let lost = before.into_iter().filter(|d| !deps.contains(d));
            self.out.journal.lost.extend(lost);
            self.out.edges.set(&name, deps);
        }
        Ok(())
    }

    fn scanned(&mut self, name: &str) -> Result<(), EngineError> {
        let held = &self.out.nodes[name];
        let (file, body) = (held.file.clone(), held.body.clone());
        let (caller, at) = held.site.clone();
        let given = held.given.clone();
        let given: Vec<&str> = given.iter().map(String::as_str).collect();
        // An unbound bareword call names a node, so it is the scan's to refuse.
        let expr = body.written.expr();
        let unbound = sva_ast::free_parameters(expr, self.graph.defaults(&file), &given, |_| true);
        if let Some(first) = unbound.into_iter().next() {
            let found = BindingFault::Unbound(file.clone(), first);
            return Err(relocate(fault(&file, None, found), &caller, at));
        }
        let mut used = BTreeSet::new();
        let mut children = Vec::new();
        let place = In {
            file: &file,
            scope: body.scope,
            at: None,
        };
        let sinks = (&mut used, &mut children);
        self.scan(expr, &mut At::of(&body.written), place, sinks)
            .map_err(|e| relocate(e, &caller, at))?;
        let filled: Vec<Bound> = self
            .out
            .scope(body.scope)
            .vars
            .iter()
            .filter(|(_, bound)| bound.scope == NO_PARAMS || self.out.scope(bound.scope).chained)
            .map(|(_, bound)| bound.clone())
            .collect();
        for value in &filled {
            let inside = In {
                scope: value.scope,
                ..place
            };
            let mut walked = At::of(&value.written);
            let sinks = (&mut used, &mut children);
            self.scan(value.written.expr(), &mut walked, inside, sinks)
                .map_err(|e| relocate(e, &caller, at))?;
        }
        let vars = &self.out.scope(body.scope).vars;
        if let Some((key, _)) = vars.iter().find(|(key, _)| !used.contains(key)) {
            return Err(fault(&caller, at, BindingFault::Unused(file, key.clone())));
        }
        self.work.extend(children);
        Ok(())
    }

    /// Sharing is decided by comparing the bindings themselves, never a name or a digest: a
    /// name a DIFFERENT tuple holds takes the next suffix, so no two tuples share a buffer.
    fn intern(
        &mut self,
        file: &str,
        mut binds: Vec<(String, Bound)>,
        (caller, span): (&str, Option<ByteSpan>),
    ) -> Result<String, EngineError> {
        let given: Vec<String> = binds.iter().map(|(name, _)| name.clone()).collect();
        let Some(defined) = self.graph.defined(file) else {
            return Err(EngineError::UnknownNode(file.to_string()));
        };
        let mut chained = Vec::new();
        let found = self.interned(file, defined, &mut binds, &mut chained, span);
        match found {
            Ok((name, true)) => {
                self.discard(chained);
                Ok(name)
            }
            Ok((name, false)) => {
                let name = self.named_new(name, defined, binds, given, (file, caller, span));
                name.inspect_err(|_| self.discard(chained))
            }
            Err(e) => {
                self.discard(chained);
                Err(e)
            }
        }
    }

    /// The name holding `binds` once the defaults they leave out are added, and whether an
    /// instance already holds it.
    fn interned(
        &mut self,
        file: &str,
        defined: &Arc<Defined>,
        binds: &mut Vec<(String, Bound)>,
        chained: &mut Vec<ScopeId>,
        span: Option<ByteSpan>,
    ) -> Result<(String, bool), EngineError> {
        // A default stands in the scope the lines above it make, in declaration order.
        for (line, (name, value)) in (1..).zip(&defined.defaults) {
            self.arithmetic_only(value, file)?;
            if binds.iter().any(|(k, _)| k == name) {
                continue;
            }
            let scope = match names_any(value, binds) {
                false => NO_PARAMS,
                true => {
                    let mut above = binds.clone();
                    above.sort_by(|a, b| a.0.cmp(&b.0));
                    let id = self.out.made(above, true);
                    chained.push(id);
                    id
                }
            };
            let written = Written::of(defined, line);
            binds.push((name.clone(), Bound { written, scope }));
        }
        binds.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, value) in binds.iter() {
            if is_reserved(key) {
                return Err(fault(file, span, BindingFault::Reserved(key.clone())));
            }
            if self
                .out
                .holds_self(value.thunk().expr, self.out.cx(value.scope))
            {
                return Err(fault(file, span, BindingFault::SelfInArgument(key.clone())));
            }
        }
        let base = self.out.display_name(file, binds);
        for attempt in 1.. {
            let name = match attempt {
                1 => base.clone(),
                _ => format!("{base}~{attempt}"),
            };
            match self.out.nodes.get(&name) {
                Some(held) if held.file == file => {
                    if self.out.same_binds(held.body.scope, binds) {
                        return Ok((name, true));
                    }
                }
                Some(_) => continue,
                None => return Ok((name, false)),
            }
        }
        unreachable!("the suffix search only ends by returning")
    }

    /// A new instance of `file` named `name`, holding `binds`.
    fn named_new(
        &mut self,
        name: String,
        defined: &Arc<Defined>,
        binds: Vec<(String, Bound)>,
        given: Vec<String>,
        (file, caller, span): (&str, &str, Option<ByteSpan>),
    ) -> Result<String, EngineError> {
        if self.out.nodes.len() >= MAX_INSTANCES {
            return Err(fault(
                file,
                span,
                BindingFault::TooManyInstances(MAX_INSTANCES),
            ));
        }
        let scope = self.out.made(binds, false);
        self.out.retain(scope);
        let held = Instance {
            body: Bound {
                written: Written::of(defined, 0),
                scope,
            },
            file: file.to_string(),
            given,
            site: (caller.to_string(), span),
            held: 0,
            scanned: false,
        };
        self.out.insert(name.clone(), held);
        self.out.journal.named.push(name.clone());
        Ok(name)
    }

    /// Frees each scope made for a tuple no instance took.
    fn discard(&mut self, made: Vec<ScopeId>) {
        for id in made.into_iter().rev() {
            self.out.drop_unheld(id);
        }
    }

    /// A default resolves before any invocation, so a `@ref` in one has no scope to bind and
    /// `self` no owner: each has to settle to a number. A term written longhand needs neither,
    /// whatever it denotes.
    fn arithmetic_only(&self, e: &Expr, file: &str) -> Result<(), EngineError> {
        let (reached, span) = match e {
            Expr::Lit(_) | Expr::Var(_) => return Ok(()),
            Expr::Bin(_, l, r) => {
                self.arithmetic_only(l, file)?;
                return self.arithmetic_only(r, file);
            }
            Expr::Ref { path, span, .. } if self.one_number(e, file, &mut Vec::new()).is_none() => {
                (format!("@{path}"), *span)
            }
            Expr::Ref { .. } => return Ok(()),
            Expr::SelfRef { span, .. } => ("self".to_string(), *span),
            Expr::Indexed { name, span, .. } => (format!("{name}[...]"), *span),
            Expr::Call { name, args, .. } if is_builtin(name) => {
                for a in args {
                    let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                    self.arithmetic_only(x, file)?;
                }
                return Ok(());
            }
            Expr::Call { name, span, .. } => (name.clone(), *span),
        };
        Err(fault(
            file,
            Some(span),
            BindingFault::DefaultNamesNoNumber(file.to_string(), reached),
        ))
    }

    /// FORMAT 15.3: a ref naming one number is that number wherever it is written, a default
    /// included. `None` for anything a caller has to pass instead.
    fn one_number(&self, e: &Expr, file: &str, open: &mut Vec<String>) -> Option<f64> {
        // A quotient at zero names no number, as `loops::plain` already answers.
        self.folded(e, file, open).filter(|v| v.is_finite())
    }

    fn folded(&self, e: &Expr, file: &str, open: &mut Vec<String>) -> Option<f64> {
        match e {
            Expr::Lit(sva_ast::Literal::Num(n)) => Some(*n),
            Expr::Lit(_) => None,
            Expr::Var(name) if name == "pi" => Some(std::f64::consts::PI),
            Expr::Var(name) => crate::vocabulary::note_hz(name),
            Expr::Bin(op, l, r) => {
                let (a, b) = (
                    self.one_number(l, file, open)?,
                    self.one_number(r, file, open)?,
                );
                match op {
                    sva_ast::BinOp::Add => Some(a + b),
                    sva_ast::BinOp::Sub => Some(a - b),
                    sva_ast::BinOp::Mul => Some(a * b),
                    sva_ast::BinOp::Div => Some(a / b),
                    sva_ast::BinOp::Mod => crate::lower::constant_modulo(a, b),
                }
            }
            Expr::Call { name, args, .. } if is_builtin(name) => {
                let (mut positional, mut named) = (Vec::new(), Vec::new());
                for arg in args {
                    match arg {
                        Arg::Pos(x) => positional.push(self.one_number(x, file, open)?),
                        Arg::Named(key, x) => {
                            named.push((key.as_str(), self.one_number(x, file, open)?));
                        }
                    }
                }
                crate::lower::constant_call(name, &positional, &named)
            }
            Expr::Ref { path, binds, .. } if binds.is_empty() => {
                let target = sva_ast::resolve_ref_path(file, path)?;
                if open.contains(&target) || !self.graph.defaults(&target).is_empty() {
                    return None;
                }
                let body = self.graph.expr(&target)?;
                open.push(target.clone());
                let held = self.one_number(body, &target, open);
                open.pop();
                held
            }
            _ => None,
        }
    }

    /// Walks a file's body once, recording which child instance every invocation names and
    /// refusing what the bindings cannot answer. Nothing is rewritten.
    fn scan(
        &mut self,
        e: &Expr,
        at: &mut At,
        place: In<'_>,
        (used, children): Sinks<'_>,
    ) -> Result<(), EngineError> {
        let In { file, scope, .. } = place;
        match e {
            Expr::Lit(_) => Ok(()),
            Expr::Var(name) => {
                if is_language_value(name) || self.indices.iter().any(|k| k == name) {
                    return Ok(());
                }
                if self.out.binds(scope, name).is_some() {
                    used.insert(name.clone());
                }
                Ok(())
            }
            Expr::Bin(_, l, r) => {
                self.down(at, Child::First, l, place, (&mut *used, &mut *children))?;
                self.down(at, Child::Right, r, place, (&mut *used, &mut *children))
            }
            Expr::SelfRef { arg, span, .. } => self.down(
                at,
                Child::First,
                arg,
                place.spanned(*span),
                (&mut *used, &mut *children),
            ),
            Expr::Indexed { name, arg, span } => {
                if self.out.binds(scope, name).is_some() {
                    used.insert(name.clone());
                }
                self.down(
                    at,
                    Child::First,
                    arg,
                    place.spanned(*span),
                    (&mut *used, &mut *children),
                )
            }
            Expr::Call { name, args, span } => {
                let place = place.spanned(*span);
                if self.out.binds(scope, name).is_some() {
                    used.insert(name.to_string());
                    let [Arg::Pos(when)] = args.as_slice() else {
                        return Err(EngineError::BadArity(name.to_string()));
                    };
                    self.down(at, Child::Arg(0), when, place, (&mut *used, &mut *children))?;
                    return self.read_parameter(name, when, place);
                }
                if name == SERIES {
                    let [
                        Arg::Pos(Expr::Var(index)),
                        Arg::Pos(lo),
                        Arg::Pos(hi),
                        Arg::Pos(term),
                    ] = args.as_slice()
                    else {
                        return Err(EngineError::BadArity(SERIES.to_string()));
                    };
                    // `sum(k, lo, hi, term)` binds `k` over the term alone.
                    self.down(at, Child::Arg(1), lo, place, (&mut *used, &mut *children))?;
                    self.down(at, Child::Arg(2), hi, place, (&mut *used, &mut *children))?;
                    self.indices.push(index.clone());
                    let scanned =
                        self.down(at, Child::Arg(3), term, place, (&mut *used, &mut *children));
                    self.indices.pop();
                    return scanned;
                }
                if let (sva_ast::INDEX, [Arg::Pos(time), ..]) = (name.as_str(), args.as_slice()) {
                    return self.down(at, Child::Arg(0), time, place, (&mut *used, &mut *children));
                }
                for (k, a) in (0..).zip(args) {
                    let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                    self.down(at, Child::Arg(k), x, place, (&mut *used, &mut *children))?;
                }
                if is_builtin(name) {
                    return Ok(());
                }
                let target = resolve_ref_path(file, name)?;
                if self.graph.expr(&target).is_none() {
                    return Err(EngineError::UnknownBuiltin(name.clone()));
                }
                let binds = call_bindings(name, args, (at, scope), (file, *span))?;
                self.invoke(e, &target, binds, place, children)
            }
            Expr::Ref {
                path,
                arg,
                binds,
                span,
                ..
            } => {
                let place = place.spanned(*span);
                self.down(at, Child::First, arg, place, (&mut *used, &mut *children))?;
                let mut bound = Vec::with_capacity(binds.len());
                for (k, (name, value)) in (0..).zip(binds) {
                    self.down(
                        at,
                        Child::Bind(k),
                        value,
                        place,
                        (&mut *used, &mut *children),
                    )?;
                    let written = at.here(Child::Bind(k));
                    bound.push((name.clone(), Bound { written, scope }));
                }
                let target = resolve_ref_path(file, path)?;
                self.invoke(e, &target, bound, place, children)
            }
        }
    }

    /// One child of what `at` stands on, scanned.
    fn down(
        &mut self,
        at: &mut At,
        child: Child,
        x: &Expr,
        place: In<'_>,
        sinks: Sinks<'_>,
    ) -> Result<(), EngineError> {
        at.path.push(child.index());
        let scanned = self.scan(x, at, place, sinks);
        at.path.pop();
        scanned
    }

    fn invoke(
        &mut self,
        site: &Expr,
        target: &str,
        binds: Vec<(String, Bound)>,
        place: In,
        children: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        let child = self.intern(target, binds, (place.file, place.at))?;
        children.push(child.clone());
        let at = std::ptr::from_ref(site) as usize;
        let scope = self.out.scopes[place.scope as usize].as_mut();
        scope
            .expect("the scope a scan stands in")
            .sites
            .insert(at, child);
        Ok(())
    }

    /// `p(t - M)` reads the argument bound to `p` at another time. Spelled without `@` so that
    /// `@` keeps meaning "a file" and a dangling ref stays a parse-time refusal.
    fn read_parameter(
        &mut self,
        name: &str,
        when: &Expr,
        place: In<'_>,
    ) -> Result<(), EngineError> {
        let bound = self
            .out
            .binds(place.scope, name)
            .expect("the caller matched this name against the same scope");
        if !self.out.is_now(when, self.out.cx(place.scope))
            && let Some(what) = self
                .out
                .position_dependent(bound.expr, self.out.cx(bound.scope))
        {
            return Err(fault(
                place.file,
                place.at,
                BindingFault::ShiftedRead(name.to_string(), what),
            ));
        }
        Ok(())
    }
}

/// The one allowed positional argument binds [`SIGNAL_PARAM`]; a second has nothing to mean.
fn call_bindings(
    name: &str,
    args: &[Arg],
    (at, scope): (&mut At, ScopeId),
    (file, span): (&str, ByteSpan),
) -> Result<Vec<(String, Bound)>, EngineError> {
    let mut binds: Vec<(String, Bound)> = Vec::new();
    for (k, a) in (0..).zip(args) {
        let bound = Bound {
            written: at.here(Child::Arg(k)),
            scope,
        };
        match a {
            Arg::Pos(_) if binds.is_empty() => {
                binds.push((SIGNAL_PARAM.to_string(), bound));
            }
            Arg::Pos(_) => return Err(EngineError::BadArity(name.to_string())),
            Arg::Named(k, _) => {
                if binds.iter().any(|(seen, _)| seen == k) {
                    return Err(fault(file, Some(span), BindingFault::Duplicate(k.clone())));
                }
                binds.push((k.clone(), bound));
            }
        }
    }
    Ok(binds)
}
