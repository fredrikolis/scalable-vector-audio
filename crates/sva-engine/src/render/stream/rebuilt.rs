// Concern: states where a changed stream's typing and table differ from a build of all it plays that carries nothing over | Non-concern: the samples either plays | IO: (Stream) -> differences

use super::{Frontier, Known, Stream, shelled};
use crate::cache::NoStore;
use crate::refs;
use crate::render::table::Kind;

impl Stream {
    /// Every node and value its own build holds otherwise than a build of all it plays, over
    /// no store, carrying nothing over, would; `None` where a term retired since its build.
    pub(crate) async fn unlike_rebuilt(&self) -> Option<Vec<String>> {
        let tys = &self.played.shell.tys;
        let typed = tys.paths().filter(|(p, _)| p.starts_with("notes#")).count();
        if typed != self.terms.count() {
            return None;
        }
        let mut render = self.config.render.clone();
        render.range.start = Some(self.driver.start);
        let walk = async |found: &mut Frontier<'_>| {
            found.walked(&mut Known::new(), &NoStore).await;
        };
        let fresh = shelled(
            &self.graph,
            &self.expr,
            &self.terms,
            &render,
            (walk, || None),
        );
        let fresh = fresh.await.expect("what a stream plays builds again");
        let (mine, theirs) = (&self.played.shell.tys, &fresh.played.shell.tys);
        let mut out = Vec::new();
        let paths = |tys: &crate::typing::Typing| -> Vec<String> {
            tys.paths().map(|(p, _)| p.to_string()).collect()
        };
        if paths(mine) != paths(theirs) || mine.len() != theirs.len() {
            out.push(format!("typed {} nodes, not {}", mine.len(), theirs.len()));
        }
        for (path, id) in theirs.paths() {
            let Some(held) = mine.id(path) else {
                continue;
            };
            let same = mine.ty(held) == theirs.ty(id)
                && mine.grid(held) == theirs.grid(id)
                && refs::identity(mine, held).ok() == refs::identity(theirs, id).ok();
            if !same {
                out.push(format!("{path} typed otherwise"));
            }
        }
        let (a, b) = (&self.driver.table, &fresh.table);
        if (a.values.len(), a.root, &a.cuts, a.moved) != (b.values.len(), b.root, &b.cuts, b.moved)
        {
            out.push(format!(
                "a table of {:?}, not {:?}",
                (a.values.len(), a.root, &a.cuts, a.moved),
                (b.values.len(), b.root, &b.cuts, b.moved)
            ));
            return Some(out);
        }
        let named = |tys: &crate::typing::Typing, v: &crate::render::table::Value| {
            v.node.map(|id| tys.name(id).to_string())
        };
        for (x, y) in a.values.iter().zip(&b.values) {
            let same = (x.key, &x.name, &x.reads, x.width, x.period, &x.switches)
                == (y.key, &y.name, &y.reads, y.width, y.period, &y.switches)
                && (x.made.support, &x.made.label, x.moved)
                    == (y.made.support, &y.made.label, y.moved)
                && named(mine, x) == named(theirs, y)
                && kind(&x.kind) == kind(&y.kind);
            if !same {
                out.push(format!("{} built otherwise", y.name));
            }
        }
        Some(out)
    }
}

fn kind(kind: &Kind) -> String {
    match kind {
        Kind::Rows(_) => "rows".to_string(),
        Kind::Program(program) => format!(
            "{:?} {:?} {} {:?}",
            program.renderer, program.start, program.own, program.alias
        ),
        Kind::Frames { window, hop } => format!("frames {window} {hop}"),
        Kind::Istft => "istft".to_string(),
        Kind::Spectrum(_) => "spectrum".to_string(),
        Kind::Stored(stored) => format!("stored {}", stored.key),
    }
}
