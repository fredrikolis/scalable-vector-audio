// Concern: folds one node's Expr into the closed form, cast or sampled op its type names | Non-concern: typing a term (sva-formula), one builtin's image (calls.rs) | IO: (Expr, Cx) -> a typed node

mod calls;
mod casts;
mod constant;
pub(crate) mod physics;
mod solvers;
mod walk;
mod waves;

use std::borrow::Cow;
use std::sync::Arc;

use sva_ast::{Address, Arg, ByteSpan, Expr, Literal};
use sva_formula::{Body, ClosedForm, Held, IndexId, NodeId, Part, Ty, Var};

use crate::cast::{Cast, Mismatch};
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::{Cx, Instances, Node};
use crate::loops::{self, SelfKind};
use crate::overload;
use crate::time::Grid;
use crate::typing::{Node as Typed, Typing, Value, When};

pub(crate) use calls::{noise_at, rand_arguments};
pub(crate) use constant::{
    constant_call, constant_modulo, constant_value, holds_infinite, number_of,
};
pub(crate) use solvers::{field, value_of};

/// One written subterm, either still inside a closed form or already a node of its own.
pub enum Piece {
    ClosedForm(Body),
    Value(NodeId),
}

/// What `self` stands for while this node's body is lowered: nothing, the zero a Neumann
/// series expands around, or a read of the node's own past, stepped, where the construct it
/// names makes the loop discrete.
enum SelfMode {
    Absent,
    Zero,
    Discrete(String),
}

/// What a `sum`'s index stands for in its term: the series' own index, or one number of a
/// finite sum written out term by term.
#[derive(Clone, Copy)]
enum Index {
    Series(IndexId),
    Term(i64),
}

pub struct Lowering<'a> {
    inst: &'a Instances,
    typing: &'a mut Typing,
    /// The node's path, the name every node it makes shares.
    node: Arc<str>,
    indices: Vec<(String, Index)>,
    mode: SelfMode,
    own: Vec<(crate::time::Q, NodeId)>,
    grid: Grid,
}

/// Dependencies are already typed, so every ref this reads answers with a decided `Ty`.
pub fn node(path: &str, inst: &Instances, typing: &mut Typing) -> Result<NodeId, EngineError> {
    match typing.id(path) {
        Some(id) if !typing.pending(id) => Ok(id),
        _ => lowered(path, inst.grid(), inst, typing),
    }
}

/// `path` on `grid`, lowered once per step with its inputs there: its step is what `sp` counts
/// in and what a stateful node steps by.
pub(crate) fn on(
    path: &str,
    grid: Grid,
    inst: &Instances,
    typing: &mut Typing,
) -> Result<NodeId, EngineError> {
    let base = typing
        .id(path)
        .ok_or_else(|| EngineError::UnknownNode(path.to_string()))?;
    if grid.is_rate() {
        return Ok(base);
    }
    if let Some(id) = typing.copy(path, grid) {
        return Ok(id);
    }
    if !typing.opened(path) {
        return Err(crate::refs::cyclic(typing, base));
    }
    let found = lowered(path, grid, inst, typing);
    typing.closed(path);
    let id = found?;
    typing.copied(path, grid, id);
    Ok(id)
}

/// A value only at the steps it takes: no closed form, and nothing a formula reads anywhere.
pub(crate) fn holds_state(typing: &Typing, id: NodeId) -> bool {
    !typing.ty(id).is_closed_form() && !crate::schedule::anywhere(typing, id)
}

fn lowered(
    path: &str,
    grid: Grid,
    inst: &Instances,
    typing: &mut Typing,
) -> Result<NodeId, EngineError> {
    let (expr, cx) = inst
        .at(path)
        .ok_or_else(|| EngineError::UnknownNode(path.to_string()))?;
    typing.lowering(path);
    let name = typing.begin(path, grid);
    let lowered = lowered_on(name, grid, (expr, cx), inst, typing);
    typing.end();
    lowered
}

fn lowered_on(
    node: Arc<str>,
    grid: Grid,
    (expr, cx): (&Expr, Cx),
    inst: &Instances,
    typing: &mut Typing,
) -> Result<NodeId, EngineError> {
    let path = &*node;
    let cx = cx.on(grid);
    let var = axis_of(inst, typing, expr, cx, path)?;
    let kind = match inst.reads_self(path) {
        false => None,
        true => {
            let discrete = crosses(inst, typing, expr, cx);
            Some(loops::classify(inst, expr, cx, path, discrete))
        }
    };
    let (mode, closed) = match kind {
        Some(SelfKind::Refuse(e)) => return Err(*e),
        Some(SelfKind::Series { gain, delay }) => (SelfMode::Zero, Some((gain, delay))),
        Some(SelfKind::Discrete { why }) => (SelfMode::Discrete(why), None),
        None => (SelfMode::Absent, None),
    };
    let mut low = Lowering {
        inst,
        typing,
        node: Arc::clone(&node),
        indices: Vec::new(),
        mode,
        own: Vec::new(),
        grid,
    };
    let piece = low.walk(expr, cx, var)?;
    let piece = match closed {
        Some((gain, delay)) => low.expand(piece, var, gain, delay)?,
        None => piece,
    };
    low.seal(piece, var, Some(path))
}

/// What in the body leaves the closed form, if anything: a series expands a closed form around
/// itself, and nothing that already reached samples can be one of its terms.
fn crosses(inst: &Instances, typing: &Typing, e: &Expr, cx: Cx) -> Option<String> {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| crosses(inst, typing, e2, cx2)) {
        return r;
    }
    match inst.node(e, cx) {
        Node::Lit(Literal::Samples(_)) => Some("a step in `sp`".to_string()),
        Node::Lit(_) | Node::Name(_) => None,
        Node::Bin(_, l, r) => crosses(inst, typing, l, cx).or_else(|| crosses(inst, typing, r, cx)),
        Node::Own {
            address: Address::Index,
            ..
        } => Some("the index read `self[...]`".to_string()),
        Node::Own { arg, .. } => crosses(inst, typing, arg, cx),
        Node::Read {
            path,
            address: Address::Index,
            ..
        } => Some(format!("the index read `@{path}[...]`")),
        Node::Signal { name, .. } => Some(format!("the index read `{name}[...]`")),
        Node::Read { path, .. } => typing
            .id(path)
            .is_some_and(|id| !typing.ty(id).is_closed_form())
            .then(|| format!("the sampled input `@{path}`")),
        Node::Call { name, args, .. } => {
            let sampled = matches!(name, "sample" | "stft" | "istft")
                || sva_ast::FINITE_DIFFERENCE.contains(&name);
            match sampled {
                true => Some(format!("the sampled input `{name}(...)`")),
                false => args.iter().find_map(|a| {
                    let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                    crosses(inst, typing, x, cx)
                }),
            }
        }
    }
}

/// The axis a node's own closed form is written on: a bare `f` or a spectrum it reads puts it in `f`,
/// and a subtree under a cast declares its own.
fn axis_of(
    inst: &Instances,
    typing: &Typing,
    e: &Expr,
    cx: Cx,
    at: &str,
) -> Result<Var, EngineError> {
    let mut seen = (false, false);
    scan_axis(inst, typing, e, cx, &mut seen);
    match seen {
        (true, true) => Err(EngineError::refused(Diagnostic {
            code: "type.domain_mismatch".to_string(),
            message: format!("`{at}` has no overload for (t, f)."),
            location: Located::at(at, None),
            help: "write ifourier on the f side to work in t, or fourier on the t side to \
                   work in f"
                .to_string(),
        })),
        (_, true) => Ok(Var::F),
        _ => Ok(Var::T),
    }
}

fn scan_axis(inst: &Instances, typing: &Typing, e: &Expr, cx: Cx, seen: &mut (bool, bool)) {
    if inst
        .follow(e, cx, |e2, cx2| scan_axis(inst, typing, e2, cx2, seen))
        .is_some()
    {
        return;
    }
    match inst.node(e, cx) {
        Node::Lit(_) => {}
        Node::Name("t") => seen.0 = true,
        Node::Name("f") => seen.1 = true,
        Node::Name(_) => {}
        Node::Bin(_, l, r) => {
            scan_axis(inst, typing, l, cx, seen);
            scan_axis(inst, typing, r, cx, seen);
        }
        Node::Own { arg, .. } | Node::Signal { arg, .. } => scan_axis(inst, typing, arg, cx, seen),
        Node::Read { path, .. } => match typing.id(path).map(|id| typing.ty(id)) {
            Some(ty) if ty.has_dual() => {}
            Some(ty) if ty.held == Held::Form(Var::T) => seen.0 = true,
            Some(ty) if ty.held == Held::Form(Var::F) => seen.1 = true,
            _ => {}
        },
        Node::Call { name, args, .. } if Cast::from_name(name).is_none() => {
            for a in args {
                let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                scan_axis(inst, typing, x, cx, seen);
            }
        }
        Node::Call { .. } => {}
    }
}

impl Lowering<'_> {
    fn here(&self, span: Option<ByteSpan>) -> Located {
        Located::at(&*self.node, span)
    }

    fn part(&mut self, f: Body, span: Option<ByteSpan>) -> Part {
        let origin = self.typing.mark(&self.node, span);
        Part::new(origin, f)
    }

    fn refused(&self, code: &str, message: String, help: &str) -> EngineError {
        self.refused_at(code, message, help, None)
    }

    fn refused_at(
        &self,
        code: &str,
        message: String,
        help: &str,
        span: Option<ByteSpan>,
    ) -> EngineError {
        EngineError::refused(Diagnostic {
            code: code.to_string(),
            message,
            location: self.here(span),
            help: help.to_string(),
        })
    }

    fn refuse(&self, call: &str, m: &Mismatch, span: Option<ByteSpan>) -> EngineError {
        EngineError::refused(overload::format_refusal(call, m, self.here(span), &[]))
    }

    /// A subterm becomes a node of its own wherever a cast or a sampled operand ends the closed form.
    fn seal(&mut self, piece: Piece, var: Var, path: Option<&str>) -> Result<NodeId, EngineError> {
        match match piece {
            // A file forwarding one ref is a node of its own; an intermediate is not.
            Piece::ClosedForm(Body::Node(id)) if path.is_none() => Piece::Value(id),
            held => held,
        } {
            Piece::Value(id) => {
                let path = path.filter(|_| self.grid.is_rate());
                let settled = path
                    .and_then(|p| self.typing.id(p))
                    .filter(|held| self.typing.pending(*held));
                match settled {
                    Some(held) => {
                        let site = self.typing.mark(&self.node, None);
                        let node = Typed {
                            name: Arc::clone(&self.node),
                            ty: self.typing.ty(id),
                            var: self.typing.var(id),
                            value: Value::Read {
                                source: id,
                                at: When::At(crate::time::Affine::NOW),
                                site,
                            },
                            grid: self.grid,
                        };
                        self.typing.settle(held, node);
                        Ok(held)
                    }
                    None => {
                        if let Some(path) = path {
                            self.typing.alias(path, id);
                        }
                        Ok(id)
                    }
                }
            }
            Piece::ClosedForm(body) => {
                let origin = self.typing.mark(&self.node, None);
                // FORMAT 15.3: a ref naming one number is that number to the typing too.
                let body = match crate::refs::fold_constants(self.typing, &body) {
                    Cow::Owned(folded) => folded,
                    Cow::Borrowed(_) => body,
                };
                if !matches!(body, Body::Const(_)) && holds_infinite(&body) {
                    return Err(
                        self.infinite("inf stands in a term that moves, which names no value")
                    );
                }
                let form = ClosedForm { var, body, origin };
                let ty = self.typing.infer_closed_form(&form)?;
                let node = Typed {
                    name: Arc::clone(&self.node),
                    ty,
                    var,
                    value: Value::ClosedForm(form),
                    grid: self.grid,
                };
                let path = path.filter(|_| self.grid.is_rate());
                match path.and_then(|p| self.typing.id(p)) {
                    Some(held) if self.typing.pending(held) => {
                        self.typing.settle(held, node);
                        Ok(held)
                    }
                    _ => Ok(self.typing.push(node, path)),
                }
            }
        }
    }

    /// `path` on the grid this node steps on.
    fn source(&mut self, path: &str) -> Result<NodeId, EngineError> {
        on(path, self.grid, self.inst, self.typing)
    }

    fn register(&mut self, value: Value, ty: Ty, var: Var) -> NodeId {
        self.typing.push(
            Typed {
                name: Arc::clone(&self.node),
                ty,
                var,
                value,
                grid: self.grid,
            },
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use sva_formula::NodeId;

    /// A closed form is exact at any instant, so a filter over it read at many shifts, each
    /// landing between two samples, leaves the form lowered once.
    #[test]
    fn a_closed_form_read_at_many_fractional_shifts_is_lowered_once() {
        let reads: Vec<String> = (1..=24)
            .map(|k| format!("@hit(t - {}s)", 0.0123 + f64::from(k) * 0.0000137))
            .collect();
        let mut files = sva_ast::Composition::new();
        files
            .insert("env", "exp(-t/0.01)*sin(2*pi*440*t)\n")
            .insert("hit", "lowpass(sample(@env), cutoff=900)\n")
            .insert("song", format!("{}\n", reads.join(" + ")));
        let g = sva_ast::load(&files).expect("a composition");
        let typing = crate::types(&g, "song").unwrap_or_else(|e| panic!("song: {e}"));
        let forms = (0..typing.len())
            .map(|n| NodeId(n as u32))
            .filter(|id| typing.name(*id) == "env")
            .count();
        assert_eq!(forms, 1, "env is lowered once, not once per shift");
    }

    /// A signal passed in as `x` reads by index as a ref does: an allpass reads its stateful
    /// input's nearest step at a delay between two samples, and `x(t - d)` reads the same one
    /// value at the whole sample `d` rounds to.
    #[test]
    fn a_parameter_read_by_index_lowers_its_signal_once() {
        const RATE: u32 = 8_000;
        let mut files = sva_ast::Composition::new();
        files
            .insert(
                "src",
                "lowpass(crop(sample(sin(2*pi*220*t)), 0s, 0.02s), cutoff=900)\n",
            )
            .insert(
                "allpass",
                "-g*x + x[idx(t - 0.00510204s)] + g*self[idx(t - 0.00510204s)]\n",
            )
            .insert(
                "timed",
                "-g*x + x(t - 0.00510204s) + g*self[idx(t - 0.00510204s)]\n",
            )
            .insert("passed", "@allpass(t, x=@src, g=0.5)\n")
            .insert("shifted", "@timed(t, x=@src, g=0.5)\n")
            .insert(
                "written",
                "-0.5*@src + @src[idx(t - 0.00510204s)] + 0.5*self[idx(t - 0.00510204s)]\n",
            );
        let g = sva_ast::load(&files).expect("a composition");
        let filters = |root: &str| {
            let typing = crate::types_at(&g, root, RATE).unwrap_or_else(|e| panic!("{root}: {e}"));
            (0..typing.len())
                .map(|n| NodeId(n as u32))
                .filter(|id| typing.name(*id) == "src")
                .filter(|id| matches!(typing.value(*id), crate::Value::Filter { .. }))
                .count()
        };
        assert_eq!(
            filters("passed"),
            1,
            "src is lowered once, on the render's grid"
        );
        assert_eq!(
            filters("shifted"),
            1,
            "a read at t - d reads src's one value at a whole-sample shift"
        );

        let config = crate::RenderConfig::seconds(RATE, 0.05);
        let out = |root: &str| -> Vec<u64> {
            let held = crate::render(&g, root, config.clone(), &crate::Tier::default())
                .unwrap_or_else(|e| panic!("{root}: {e}"));
            let id = held.id(root).expect("the root");
            let plane = held.output(id).expect("a buffer").plane(0).to_vec();
            assert!(plane.iter().any(|v| *v != 0.0), "silence tests nothing");
            plane.iter().map(|v| v.to_bits()).collect()
        };
        assert_eq!(
            out("passed"),
            out("written"),
            "x[i] reads what @src[i] reads, bit for bit"
        );
    }
}
