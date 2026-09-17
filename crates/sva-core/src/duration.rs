// Concern: finds the widest crop() window a node states about itself | Non-concern: producing that arrangement (sva-ast's desugaring) | IO: (&Graph, node) -> Crop

use std::collections::HashSet;

use sva_ast::{Arg, BinOp, Expr, Graph, Literal};

/// A start below zero is pre-roll and an end past the render is length.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Crop {
    pub start: Option<f64>,
    pub end: Option<f64>,
}

impl Crop {
    fn widen(self, other: Crop) -> Crop {
        Crop {
            start: pick(self.start, other.start, f64::min),
            end: pick(self.end, other.end, f64::max),
        }
    }
}

fn pick(a: Option<f64>, b: Option<f64>, f: fn(f64, f64) -> f64) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(f(a, b)),
        (a, None) => a,
        (None, b) => b,
    }
}

pub fn widest_crop(graph: &Graph, node: &str) -> Crop {
    Crop {
        start: earliest(graph, node),
        end: furthest_end(graph, node),
    }
}

/// A `crop` reaching before zero is global time wherever it is written.
fn earliest(graph: &Graph, node: &str) -> Option<f64> {
    let mut queue = vec![node.to_string()];
    let mut seen: HashSet<String> = HashSet::from([node.to_string()]);
    let mut earliest = None;
    while let Some(path) = queue.pop() {
        let Some(expr) = graph.expr(&path) else {
            continue;
        };
        let mut reached = Vec::new();
        earliest = pick(earliest, starts(expr, &mut reached), f64::min);
        for name in reached {
            let Some(target) = sva_ast::resolve_ref_path(&path, &name) else {
                continue;
            };
            if seen.insert(target.clone()) {
                queue.push(target);
            }
        }
    }
    earliest
}

fn starts(expr: &Expr, reached: &mut Vec<String>) -> Option<f64> {
    match expr {
        Expr::Bin(_, l, r) => pick(starts(l, reached), starts(r, reached), f64::min),
        Expr::Call { name, args, .. } => {
            if !sva_engine::is_builtin(name) {
                reached.push(name.clone());
            }
            let mine = (name == "crop").then(|| literal(args.get(1))).flatten();
            args.iter().fold(mine, |acc, a| {
                let (Arg::Pos(e) | Arg::Named(_, e)) = a;
                pick(acc, starts(e, reached), f64::min)
            })
        }
        Expr::Ref {
            path, arg, binds, ..
        } => {
            reached.push(path.clone());
            binds
                .iter()
                .map(|(_, e)| e)
                .chain([&**arg])
                .fold(None, |acc, e| pick(acc, starts(e, reached), f64::min))
        }
        Expr::SelfRef { arg, .. } => starts(arg, reached),
        Expr::Lit(_) | Expr::Var(_) => None,
    }
}

/// Follows what shares `node`'s time base: a ref at bare `t`, and a bareword invocation.
fn furthest_end(graph: &Graph, node: &str) -> Option<f64> {
    let mut queue = vec![node.to_string()];
    let mut seen: HashSet<String> = HashSet::from([node.to_string()]);
    let mut furthest = Crop::default();

    while let Some(path) = queue.pop() {
        let Some(expr) = graph.expr(&path) else {
            continue;
        };
        let mut aliases = Vec::new();
        furthest = furthest.widen(walk(expr, &mut aliases));
        for alias in aliases {
            let Some(target) = sva_ast::resolve_ref_path(&path, &alias) else {
                continue;
            };
            if seen.insert(target.clone()) {
                queue.push(target);
            }
        }
    }
    furthest.end
}

/// A prefix minus is parsed as `0 - n`, and a pre-roll bound is written with one.
fn literal(arg: Option<&Arg>) -> Option<f64> {
    let Some(Arg::Pos(e)) = arg else {
        return None;
    };
    match e {
        Expr::Lit(Literal::Num(v)) => Some(*v),
        Expr::Bin(BinOp::Sub, l, r) => match (&**l, &**r) {
            (Expr::Lit(Literal::Num(z)), Expr::Lit(Literal::Num(v))) if *z == 0.0 => Some(-v),
            _ => None,
        },
        _ => None,
    }
}

fn walk(expr: &Expr, aliases: &mut Vec<String>) -> Crop {
    match expr {
        Expr::Bin(_, l, r) => walk(l, aliases).widen(walk(r, aliases)),
        Expr::Call { name, args, .. } if name == "crop" => Crop {
            start: literal(args.get(1)),
            end: literal(args.get(2)),
        },
        Expr::Call { name, args, .. } => {
            if !sva_engine::is_builtin(name) {
                aliases.push(name.clone());
            }
            args.iter().fold(Crop::default(), |acc, a| {
                let (Arg::Pos(e) | Arg::Named(_, e)) = a;
                acc.widen(walk(e, aliases))
            })
        }
        Expr::Ref { path, arg, .. } => {
            if **arg == Expr::Var("t".to_string()) {
                aliases.push(path.clone());
            }
            Crop::default()
        }
        Expr::Lit(_) | Expr::Var(_) | Expr::SelfRef { .. } => Crop::default(),
    }
}
