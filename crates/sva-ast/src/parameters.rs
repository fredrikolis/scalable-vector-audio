// Concern: names each parameter a node reads that neither its default lines nor its caller bind | Non-concern: the values bound, which names the language owns | IO: (body, defaults, given) -> names

use crate::expr::{Arg, Expr, INDEX, SERIES};
use crate::graph::{Graph, resolve_ref_path};
use crate::vocabulary::{is_builtin, is_language_value};

/// In first-read order, the body before the default lines a caller left out, each line seeing
/// `given` and the lines above it. `given` names what the caller binds; `is_node` says whether
/// a bareword call to an unbound name invokes a node.
pub fn free_parameters(
    body: &Expr,
    defaults: &[(String, Expr)],
    given: &[&str],
    is_node: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut bound: Vec<&str> = given.to_vec();
    bound.extend(defaults.iter().map(|(name, _)| name.as_str()));
    let mut walk = Walk {
        bound: bound.clone(),
        indices: Vec::new(),
        is_node: &is_node,
        found: Vec::new(),
    };
    walk.expr(body);
    for (at, (name, value)) in defaults.iter().enumerate() {
        if given.contains(&name.as_str()) {
            continue;
        }
        walk.bound = bound[..given.len() + at].to_vec();
        walk.expr(value);
    }
    walk.found
}

impl Graph {
    /// [`free_parameters`] of the node at `path`, a bareword call naming a node this graph
    /// defines.
    pub fn free_parameters(&self, path: &str) -> Vec<String> {
        let Some(body) = self.expr(path) else {
            return Vec::new();
        };
        let is_node = |name: &str| resolve_ref_path(path, name).is_some_and(|n| self.defines(&n));
        free_parameters(body, self.defaults(path), &[], is_node)
    }
}

struct Walk<'a, F: Fn(&str) -> bool> {
    bound: Vec<&'a str>,
    indices: Vec<&'a str>,
    is_node: &'a F,
    found: Vec<String>,
}

impl<'a, F: Fn(&str) -> bool> Walk<'a, F> {
    fn free(&mut self, name: &str) {
        if !self.found.iter().any(|held| held == name) {
            self.found.push(name.to_string());
        }
    }

    fn args(&mut self, args: &'a [Arg]) {
        for a in args {
            let (Arg::Pos(x) | Arg::Named(_, x)) = a;
            self.expr(x);
        }
    }

    fn expr(&mut self, e: &'a Expr) {
        match e {
            Expr::Lit(_) => {}
            Expr::Var(name) => {
                let held =
                    self.bound.contains(&name.as_str()) || self.indices.contains(&name.as_str());
                if !held && !is_language_value(name) {
                    self.free(name);
                }
            }
            Expr::Bin(_, l, r) => {
                self.expr(l);
                self.expr(r);
            }
            Expr::SelfRef { arg, .. } => self.expr(arg),
            Expr::Indexed { name, arg, .. } => {
                if !self.bound.contains(&name.as_str()) {
                    self.free(name);
                }
                self.expr(arg);
            }
            Expr::Ref { arg, binds, .. } => {
                self.expr(arg);
                for (_, value) in binds {
                    self.expr(value);
                }
            }
            Expr::Call { name, args, .. } if self.bound.contains(&name.as_str()) => self.args(args),
            Expr::Call { name, args, .. } if name == SERIES => match args.as_slice() {
                [
                    Arg::Pos(Expr::Var(index)),
                    Arg::Pos(lo),
                    Arg::Pos(hi),
                    Arg::Pos(term),
                ] => {
                    self.expr(lo);
                    self.expr(hi);
                    self.indices.push(index);
                    self.expr(term);
                    self.indices.pop();
                }
                _ => self.args(args),
            },
            Expr::Call { name, args, .. } if name == INDEX => match args.first() {
                Some(Arg::Pos(time)) => self.expr(time),
                _ => self.args(args),
            },
            Expr::Call { name, args, .. } => {
                self.args(args);
                if !is_builtin(name) && !(self.is_node)(name) {
                    self.free(name);
                }
            }
        }
    }
}
