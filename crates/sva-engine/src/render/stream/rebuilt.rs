// Concern: states where a changed stream's typing and table differ from a build of all it plays that carries nothing over | Non-concern: the samples either plays | IO: (Stream) -> differences

use std::collections::BTreeMap;

use super::Stream;
use crate::refs;
use crate::render::table::{Kind, Table, Value};
use crate::render::terms::{NOTES, is_term};
use crate::render::world::{Root, STREAMED, Walked, Wanted, World};
use crate::typing::Typing;

impl Stream {
    /// Every node and value its own build holds otherwise than a build of all it plays, over
    /// no store, carrying nothing over, would; `None` where a term retired since its build.
    pub(crate) async fn unlike_rebuilt(&self) -> Option<Vec<String>> {
        let mine = &self.world.typing;
        let typed = mine.paths().filter(|(p, _)| is_term(p)).count();
        if typed != self.terms.count() {
            return None;
        }
        let mut graph = self.world.graph.clone();
        graph.set(STREAMED, None);
        if self.world.notes {
            graph.set(NOTES, None);
        }
        let render = &self.config.render;
        let mut fresh = World::over(&graph, render.rate, self.world.notes);
        let wanted = Wanted {
            root: Root::Streamed(&self.expr),
            terms: &self.terms,
            term: None,
            from: None,
            whole: false,
        };
        let mut missed = |_: &str, _| crate::cache::Known::Miss;
        match fresh.plan(&wanted, render, &mut missed) {
            Ok(Walked::Planned(_)) => {}
            Ok(Walked::Asks(_)) => unreachable!("every key a miss"),
            Err(e) => panic!("what a stream plays plans: {e}"),
        }
        let theirs = &mut fresh.typing;
        let id = theirs.id(STREAMED).expect("the root");
        let mut table = Table::new(&self.config.render.profile);
        let root = table.grow(theirs, id, &BTreeMap::new()).expect("a table");
        let mut out = Vec::new();
        typings(mine, theirs, &mut out);
        let at = self.driver.table.root;
        tables(
            (&self.driver.table, at, mine),
            (&table, root, theirs),
            &mut out,
        );
        Some(out)
    }
}

fn typings(mine: &Typing, theirs: &Typing, out: &mut Vec<String>) {
    let paths = |tys: &Typing| -> Vec<String> { tys.paths().map(|(p, _)| p.to_string()).collect() };
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
}

/// Each value the two roots reach, by key: what each holds, and the keys it reads.
fn tables(
    (a, a_root, a_tys): (&Table, usize, &Typing),
    (b, b_root, b_tys): (&Table, usize, &Typing),
    out: &mut Vec<String>,
) {
    let (mine, theirs) = (reached(a, a_root), reached(b, b_root));
    let shape = |table: &Table, held: &BTreeMap<_, usize>, root: usize| {
        (held.len(), table.moved(), table.values[root].key)
    };
    let (x, y) = (shape(a, &mine, a_root), shape(b, &theirs, b_root));
    if x != y {
        out.push(format!("a table of {x:?}, not {y:?}"));
        return;
    }
    for (key, y) in &theirs {
        let Some(x) = mine.get(key) else {
            out.push(format!("{} built otherwise", b.values[*y].name));
            continue;
        };
        let (x, y) = (&a.values[*x], &b.values[*y]);
        let keys = |table: &Table, v: &Value| -> Vec<_> {
            v.reads.iter().map(|r| table.values[*r].key).collect()
        };
        let named = |tys: &Typing, v: &Value| v.node.map(|id| tys.name(id).to_string());
        let same = (
            &x.name,
            x.width,
            x.period,
            &x.switches,
            x.whole,
            &x.label,
            x.moved,
        ) == (
            &y.name,
            y.width,
            y.period,
            &y.switches,
            y.whole,
            &y.label,
            y.moved,
        ) && keys(a, x) == keys(b, y)
            && named(a_tys, x) == named(b_tys, y)
            && kind(&x.kind) == kind(&y.kind);
        if !same {
            out.push(format!("{} built otherwise", y.name));
        }
    }
}

/// Each value `root` reaches, by key.
fn reached(table: &Table, root: usize) -> BTreeMap<crate::render::table::Key, usize> {
    let (mut held, mut open) = (BTreeMap::new(), vec![root]);
    while let Some(at) = open.pop() {
        let value = &table.values[at];
        if held.insert(value.key, at).is_none() {
            open.extend(value.reads.iter().copied());
        }
    }
    held
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
        Kind::Resident(stored) => format!("stored {}", stored.key),
    }
}
