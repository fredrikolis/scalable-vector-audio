// Concern: walks every reachable file once and names one instance per argument tuple | Non-concern: what an instance holds (mod.rs), reading one back (resolved.rs) | IO: (&Graph, roots) -> Instances

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use sva_ast::{Arg, ByteSpan, Defined, Expr, Graph};

use crate::error::{BindingFault, EngineError};
use crate::instantiate::{
    Bound, Instances, MAX_INSTANCES, NO_PARAMS, SIGNAL_PARAM, Scope, ScopeId, Written,
    resolve_ref_path,
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
    let mut b = Builder {
        graph,
        out: Instances {
            scopes: vec![Scope::new(Vec::new())],
            sites: HashMap::new(),
            nodes: BTreeMap::new(),
            origin: BTreeMap::new(),
            own_terms: BTreeMap::new(),
            root: String::new(),
            time: Expr::Var("t".to_string()),
            rate,
        },
        site: BTreeMap::new(),
        indices: Vec::new(),
        chained: BTreeSet::new(),
        given: BTreeMap::new(),
    };
    let mut named: Vec<String> = Vec::with_capacity(roots.len());
    for root in roots {
        let name = b.intern(root, Vec::new(), None)?;
        b.site
            .entry(name.clone())
            .or_insert_with(|| (root.to_string(), None));
        b.out.own_terms.insert(root.clone(), name.clone());
        named.push(name);
    }
    b.out.root = named.first().cloned().unwrap_or_default();

    let mut scanned: BTreeSet<String> = BTreeSet::new();
    let mut work: Vec<String> = named.iter().rev().cloned().collect();
    while let Some(name) = work.pop() {
        if !scanned.insert(name.clone()) {
            continue;
        }
        let file = b.out.origin[&name].clone();
        let held = b.out.nodes[&name].clone();
        let (caller, at) = b.site[&name].clone();
        let given: Vec<&str> = b.given[&name].iter().map(String::as_str).collect();
        // An unbound bareword call names a node, so it is the scan's to refuse.
        let body = held.written.expr();
        let unbound = sva_ast::free_parameters(body, graph.defaults(&file), &given, |_| true);
        if let Some(first) = unbound.into_iter().next() {
            let found = BindingFault::Unbound(file.clone(), first);
            return Err(relocate(fault(&file, None, found), &caller, at));
        }
        let mut used = BTreeSet::new();
        let mut children = Vec::new();
        let place = In {
            file: &file,
            scope: held.scope,
            at: None,
        };
        let sinks = (&mut used, &mut children);
        b.scan(body, &mut At::of(&held.written), place, sinks)
            .map_err(|e| relocate(e, &caller, at))?;
        let filled: Vec<Bound> = b.out.scopes[held.scope as usize]
            .vars
            .iter()
            .filter(|(_, bound)| bound.scope == NO_PARAMS || b.chained.contains(&bound.scope))
            .map(|(_, bound)| bound.clone())
            .collect();
        for value in &filled {
            let inside = In {
                scope: value.scope,
                ..place
            };
            let mut walked = At::of(&value.written);
            let sinks = (&mut used, &mut children);
            b.scan(value.written.expr(), &mut walked, inside, sinks)
                .map_err(|e| relocate(e, &caller, at))?;
        }
        let keys: Vec<String> = b.out.scopes[held.scope as usize]
            .vars
            .iter()
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            if !used.contains(&key) {
                return Err(fault(
                    &caller,
                    at,
                    BindingFault::Unused(file.clone(), key.clone()),
                ));
            }
        }
        work.extend(children);
    }

    Ok((b.out, named))
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

    fn here(&self) -> Written {
        Written {
            defined: Arc::clone(&self.defined),
            path: self.path.as_slice().into(),
        }
    }
}

/// Whether an expression names any of the parameters bound so far.
fn names_any(e: &Expr, binds: &[(String, Bound)]) -> bool {
    binds.iter().any(|(name, _)| sva_ast::occurs_free(e, name))
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

struct Builder<'g> {
    graph: &'g Graph,
    out: Instances,
    site: BTreeMap<String, (String, Option<ByteSpan>)>,
    /// The series indices in scope, innermost last.
    indices: Vec<String>,
    /// The scopes a default stands in, one per default that names a line above it.
    chained: BTreeSet<ScopeId>,
    /// The names each instance's caller binds.
    given: BTreeMap<String, Vec<String>>,
}

impl Builder<'_> {
    /// Sharing is decided by comparing the bindings themselves, never a name or a digest: a
    /// name a DIFFERENT tuple holds takes the next suffix, so no two tuples share a buffer.
    fn intern(
        &mut self,
        file: &str,
        mut binds: Vec<(String, Bound)>,
        span: Option<ByteSpan>,
    ) -> Result<String, EngineError> {
        let given: Vec<String> = binds.iter().map(|(name, _)| name.clone()).collect();
        let Some(defined) = self.graph.defined(file) else {
            return Err(EngineError::UnknownNode(file.to_string()));
        };
        // A default stands in the scope the lines above it make, in declaration order.
        for (line, (name, value)) in (1..).zip(&defined.defaults) {
            self.arithmetic_only(value, file)?;
            if binds.iter().any(|(k, _)| k == name) {
                continue;
            }
            let scope = match names_any(value, &binds) {
                false => NO_PARAMS,
                true => {
                    let id = self.out.scopes.len() as ScopeId;
                    let mut above = binds.clone();
                    above.sort_by(|a, b| a.0.cmp(&b.0));
                    self.out.scopes.push(Scope::new(above));
                    self.chained.insert(id);
                    id
                }
            };
            let written = Written::of(defined, line, &[]);
            binds.push((name.clone(), Bound { written, scope }));
        }
        binds.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, value) in &binds {
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

        let base = self.out.display_name(file, &binds);
        for attempt in 1.. {
            let name = if attempt == 1 {
                base.clone()
            } else {
                format!("{base}~{attempt}")
            };
            match self.out.origin.get(&name) {
                Some(taken) if taken == file => {
                    if self.out.same_binds(self.out.nodes[&name].scope, &binds) {
                        return Ok(name);
                    }
                }
                Some(_) => continue,
                None => {
                    if self.out.origin.len() >= MAX_INSTANCES {
                        return Err(fault(
                            file,
                            span,
                            BindingFault::TooManyInstances(MAX_INSTANCES),
                        ));
                    }
                    let scope = self.out.scopes.len() as ScopeId;
                    self.out.scopes.push(Scope::new(binds));
                    self.out.origin.insert(name.clone(), file.to_string());
                    self.given.insert(name.clone(), given);
                    let written = Written::of(defined, 0, &[]);
                    self.out
                        .nodes
                        .insert(name.clone(), Bound { written, scope });
                    return Ok(name);
                }
            }
        }
        unreachable!("the suffix search only ends by returning")
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
                self.down(at, 0, l, place, (&mut *used, &mut *children))?;
                self.down(at, 1, r, place, (&mut *used, &mut *children))
            }
            Expr::SelfRef { arg, span, .. } => self.down(
                at,
                0,
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
                    0,
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
                    self.down(at, 0, when, place, (&mut *used, &mut *children))?;
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
                    self.down(at, 1, lo, place, (&mut *used, &mut *children))?;
                    self.down(at, 2, hi, place, (&mut *used, &mut *children))?;
                    self.indices.push(index.clone());
                    let scanned = self.down(at, 3, term, place, (&mut *used, &mut *children));
                    self.indices.pop();
                    return scanned;
                }
                if let (sva_ast::INDEX, [Arg::Pos(time), ..]) = (name.as_str(), args.as_slice()) {
                    return self.down(at, 0, time, place, (&mut *used, &mut *children));
                }
                for (k, a) in (0..).zip(args) {
                    let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                    self.down(at, k, x, place, (&mut *used, &mut *children))?;
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
                self.down(at, 0, arg, place, (&mut *used, &mut *children))?;
                let mut bound = Vec::with_capacity(binds.len());
                for (k, (name, value)) in (1..).zip(binds) {
                    self.down(at, k, value, place, (&mut *used, &mut *children))?;
                    at.path.push(k);
                    bound.push((
                        name.clone(),
                        Bound {
                            written: at.here(),
                            scope,
                        },
                    ));
                    at.path.pop();
                }
                let target = resolve_ref_path(file, path)?;
                self.invoke(e, &target, bound, place, children)
            }
        }
    }

    /// The `k`th child of what `at` stands on, scanned.
    fn down(
        &mut self,
        at: &mut At,
        k: u32,
        x: &Expr,
        place: In<'_>,
        sinks: Sinks<'_>,
    ) -> Result<(), EngineError> {
        at.path.push(k);
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
        let child = self.intern(target, binds, place.at)?;
        self.site
            .entry(child.clone())
            .or_insert_with(|| (place.file.to_string(), place.at));
        children.push(child.clone());
        self.out
            .sites
            .insert((std::ptr::from_ref(site) as usize, place.scope), child);
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
        at.path.push(k);
        let bound = Bound {
            written: at.here(),
            scope,
        };
        at.path.pop();
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
