// Concern: writes a resolved walk back out, as source text or as a substituted Expr | Non-concern: the walk itself (mod.rs) | IO: (&Expr, Cx) -> String, Expr

use std::collections::BTreeMap;

use sva_ast::{Arg, Expr};

use crate::instantiate::{Cx, Instances, Node, Thunk};

impl<'g> Instances<'g> {
    pub(crate) fn display_name(&self, file: &str, binds: &[(String, Thunk<'g>)]) -> String {
        if binds.is_empty() {
            return file.to_string();
        }
        let args: Vec<String> = binds
            .iter()
            .map(|(k, v)| format!("{k}={}", self.render(v.expr, Cx::root(v.scope))))
            .collect();
        format!("{file}({})", args.join(", "))
    }

    /// The text an instance name and a refusal quote carry, through the one printer.
    pub fn render(&self, e: &Expr, cx: Cx) -> String {
        sva_ast::render_expr(&self.copy(e, cx))
    }

    /// Every instance's body with each parameter already standing for what it was bound to.
    pub fn exprs(&self) -> BTreeMap<String, Expr> {
        self.nodes
            .keys()
            .map(|path| {
                let (e, cx) = self.at(path).expect("a key of the map it indexes");
                (path.clone(), self.copy(e, cx))
            })
            .collect()
    }

    fn copy(&self, e: &Expr, cx: Cx) -> Expr {
        if let Some(r) = self.follow(e, cx, |e2, cx2| self.copy(e2, cx2)) {
            return r;
        }
        match self.node(e, cx) {
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
                path: path.to_string(),
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
