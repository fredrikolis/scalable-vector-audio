// Concern: folds one node's Expr into the closed form, cast or sampled op its type names | Non-concern: typing a term (sva-formula), one builtin's image (calls.rs) | IO: (Expr, Cx) -> a typed node

mod calls;
mod casts;
mod constant;
pub(crate) mod physics;
mod solvers;
mod walk;
mod waves;

use sva_ast::{Arg, ByteSpan, Expr, Literal};
use sva_formula::{Body, ClosedForm, Held, IndexId, NodeId, Part, Ty, Var};

use crate::cast::{Cast, Mismatch};
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::{Cx, Instances, Node};
use crate::loops::{self, Delay, SelfKind};
use crate::offset::Offset;
use crate::overload;
use crate::typing::{Node as Typed, Typing, Value};

pub(crate) use constant::{constant_call, constant_modulo, constant_value};

/// One written subterm, either still inside a closed form or already a node of its own.
pub enum Piece {
    ClosedForm(Body),
    Value(NodeId),
}

/// What `self(...)` stands for while this node's body is lowered: nothing, the zero a
/// Neumann series expands around, or one read of the node's own output on the grid.
#[derive(Clone, Copy, PartialEq)]
enum SelfMode {
    Absent,
    Zero,
    Sampled,
}

pub struct Lowering<'a, 'g> {
    inst: &'a Instances<'g>,
    typing: &'a mut Typing,
    node: &'a str,
    indices: Vec<(String, IndexId)>,
    mode: SelfMode,
    own: Vec<(Delay, NodeId)>,
}

/// Dependencies are already typed, so every ref this reads answers with a decided `Ty`.
pub fn node(path: &str, inst: &Instances, typing: &mut Typing) -> Result<NodeId, EngineError> {
    match typing.id(path) {
        Some(id) if !typing.pending(id) => return Ok(id),
        _ => {}
    }
    let (expr, cx) = inst
        .at(path)
        .ok_or_else(|| EngineError::UnknownNode(path.to_string()))?;
    let var = axis_of(inst, typing, expr, cx, path)?;
    let kind = match inst.reads_self(path) {
        false => None,
        true => Some(loops::classify(inst, expr, cx, path)),
    };
    let closed = match kind {
        Some(SelfKind::Refuse(e)) => return Err(*e),
        Some(SelfKind::Series { gain, delay }) if !crosses(inst, typing, expr, cx) => {
            Some((gain, delay))
        }
        Some(SelfKind::Series { .. }) | Some(SelfKind::Sampled) => {
            return sampled_loop(path, inst, typing, expr, cx, var);
        }
        None => None,
    };
    let mut low = Lowering {
        inst,
        typing,
        node: path,
        indices: Vec::new(),
        mode: match closed {
            Some(_) => SelfMode::Zero,
            None => SelfMode::Absent,
        },
        own: Vec::new(),
    };
    let piece = low.walk(expr, cx, var)?;
    let piece = match closed {
        Some((gain, delay)) => low.expand(piece, var, gain, delay)?,
        None => piece,
    };
    low.seal(piece, var, Some(path))
}

fn sampled_loop(
    path: &str,
    inst: &Instances,
    typing: &mut Typing,
    expr: &Expr,
    cx: Cx,
    var: Var,
) -> Result<NodeId, EngineError> {
    let mut low = Lowering {
        inst,
        typing,
        node: path,
        indices: Vec::new(),
        mode: SelfMode::Sampled,
        own: Vec::new(),
    };
    let piece = low.walk(expr, cx, var)?;
    low.seal(piece, var, Some(path))
}

/// Whether the body leaves the closed form: a series expands a closed form around itself, and nothing that
/// already reached samples can be one of its terms.
fn crosses(inst: &Instances, typing: &Typing, e: &Expr, cx: Cx) -> bool {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| crosses(inst, typing, e2, cx2)) {
        return r;
    }
    match inst.node(e, cx) {
        Node::Lit(Literal::Samples(_)) => true,
        Node::Lit(_) | Node::Name(_) => false,
        Node::Bin(_, l, r) => crosses(inst, typing, l, cx) || crosses(inst, typing, r, cx),
        Node::Own { .. } => false,
        Node::Read { path, .. } => typing
            .id(path)
            .is_some_and(|id| !typing.ty(id).is_closed_form()),
        Node::Call { name, args, .. } => {
            matches!(name, "sample" | "stft" | "istft")
                || crate::overload::FINITE_DIFFERENCE.contains(&name)
                || args.iter().any(|a| {
                    let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                    crosses(inst, typing, x, cx)
                })
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
        Node::Own { arg, .. } => scan_axis(inst, typing, arg, cx, seen),
        Node::Read { path, .. } => match typing.id(path).map(|id| typing.ty(id)) {
            Some(ty) if ty.has_dual() => {}
            Some(ty) if ty.held == Held::Form(Var::T) => seen.0 = true,
            Some(ty) if ty.held == Held::Form(Var::F) => seen.1 = true,
            _ => {}
        },
        Node::Call { name, args, .. } if Cast::from_name(name, &[]).is_none() => {
            for a in args {
                let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                scan_axis(inst, typing, x, cx, seen);
            }
        }
        Node::Call { .. } => {}
    }
}

impl<'g> Lowering<'_, 'g> {
    fn here(&self, span: Option<ByteSpan>) -> Located {
        Located::at(self.node, span)
    }

    fn part(&mut self, f: Body, span: Option<ByteSpan>) -> Part {
        let origin = self.typing.mark(self.here(span));
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
                let settled = path
                    .and_then(|p| self.typing.id(p))
                    .filter(|held| self.typing.pending(*held));
                match settled {
                    Some(held) => {
                        let site = self.typing.mark(self.here(None));
                        let node = Typed {
                            name: self.node.to_string(),
                            ty: self.typing.ty(id),
                            var: self.typing.var(id),
                            value: Value::Read {
                                source: id,
                                at: Offset::Steps(0),
                                site,
                            },
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
                let origin = self.typing.mark(self.here(None));
                // FORMAT 15.3: a ref naming one number is that number to the typing too.
                let body = crate::refs::fold_constants(self.typing, &body);
                let form = ClosedForm { var, body, origin };
                let ty = self.typing.infer_closed_form(&form)?;
                let node = Typed {
                    name: path.unwrap_or(self.node).to_string(),
                    ty,
                    var,
                    value: Value::ClosedForm(form),
                };
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

    fn register(&mut self, value: Value, ty: Ty, var: Var) -> NodeId {
        let name = self.node.to_string();
        self.typing.push(
            Typed {
                name,
                ty,
                var,
                value,
            },
            None,
        )
    }
}
