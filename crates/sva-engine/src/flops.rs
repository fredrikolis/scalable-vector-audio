// Concern: counts what a render costs in operations, per value, off the table's own plan | Non-concern: running any of it (render/) | IO: (&Render) -> Tree, Work

use std::collections::BTreeSet;

use crate::render::Render;
use crate::render::table::{Kind, Table};

/// `subtree` is what the holder pays for this value; `shared` marks one an earlier row already
/// paid for, read again at no cost.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub depth: usize,
    pub node: String,
    pub own: u128,
    pub subtree: u128,
    pub percent: f64,
    pub route: &'static str,
    pub shared: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tree {
    pub total: u128,
    pub budget: u128,
    pub rows: Vec<Row>,
}

/// What a stream or a render did, counted exactly and alike on every machine: the samples
/// written, the price, and the waves summed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    pub samples: u64,
    pub priced_flops: u128,
    pub waves: Option<u128>,
}

struct Node {
    at: usize,
    subtree: u128,
    own: u128,
    shared: bool,
    children: Vec<Node>,
}

/// A row under this share of the whole folds into its level's `others`, unless it is `shared`.
const FOLD_PERCENT: u128 = 1;

/// A child this close to its parent restates it, and is skipped where it owns nothing.
const PASS_THROUGH_PERCENT: u128 = 99;

/// Every value once, however many read it.
pub fn total(render: &Render) -> u128 {
    render
        .table
        .as_ref()
        .map_or(0, |table| table.planned.iter().sum())
}

pub fn tree(render: &Render) -> Tree {
    tree_at(render, render.root)
}

pub fn tree_at(render: &Render, node: sva_formula::NodeId) -> Tree {
    let Some((table, at)) = render
        .table
        .as_ref()
        .and_then(|table| Some((table, table.of(node)?)))
    else {
        return Tree {
            total: 0,
            budget: render.config.flop_budget,
            rows: Vec::new(),
        };
    };
    let root = grow(table, at, &mut BTreeSet::new());
    let total = root.subtree;
    let mut rows = Vec::new();
    emit(&root, table, 0, total, &mut rows);
    Tree {
        total,
        budget: render.config.flop_budget,
        rows,
    }
}

/// The root restates the whole, so the refusal names the costliest row under it.
pub fn dominating(tree: &Tree) -> Option<&Row> {
    let root = tree.rows.first()?;
    tree.rows
        .iter()
        .skip(1)
        .max_by_key(|row| row.subtree)
        .or(Some(root))
}

fn grow(table: &Table, at: usize, walked: &mut BTreeSet<usize>) -> Node {
    walked.insert(at);
    let own = table.planned[at];
    let mut reads = table.values[at].reads.clone();
    reads.dedup();
    let children: Vec<Node> = reads
        .into_iter()
        .map(|read| match walked.contains(&read) {
            true => Node {
                at: read,
                subtree: 0,
                own: 0,
                shared: true,
                children: Vec::new(),
            },
            false => grow(table, read, walked),
        })
        .collect();
    Node {
        at,
        subtree: own + children.iter().map(|c| c.subtree).sum::<u128>(),
        own,
        shared: false,
        children,
    }
}

/// The rule its label names, or a program's own where it has none.
fn route(table: &Table, at: usize) -> &'static str {
    let named = table.values[at].label.as_ref().map(|l| l.rule().as_str());
    match &table.values[at].kind {
        Kind::Rows(_) | Kind::Program(_) if named.is_some() => named.expect("a label"),
        Kind::Rows(_) => "rows",
        Kind::Program(_) => "sampled program",
        Kind::Frames { .. } => "short-time transform",
        Kind::Istft => "inverse short-time transform",
        Kind::Spectrum(_) => "inverse spectrum",
        Kind::Resident { .. } => "stored",
    }
}

fn emit(node: &Node, table: &Table, depth: usize, total: u128, rows: &mut Vec<Row>) {
    rows.push(Row {
        depth,
        node: table.values[node.at].name.clone(),
        own: node.own,
        subtree: node.subtree,
        percent: percent(node.subtree, total),
        route: route(table, node.at),
        shared: node.shared,
    });
    children(node, table, depth + 1, total, rows);
}

fn children(node: &Node, table: &Table, depth: usize, total: u128, rows: &mut Vec<Row>) {
    let mut held: Vec<&Node> = node.children.iter().collect();
    held.sort_by_key(|c| std::cmp::Reverse(c.subtree));
    let (shown, folded): (Vec<&Node>, Vec<&Node>) = held
        .into_iter()
        .partition(|c| c.shared || c.subtree * 100 >= total * FOLD_PERCENT);
    for child in shown {
        let restates = child.subtree * 100 >= node.subtree * PASS_THROUGH_PERCENT
            && child.own * 100 < total * FOLD_PERCENT;
        match restates && !child.children.is_empty() {
            true => children(child, table, depth, total, rows),
            false => emit(child, table, depth, total, rows),
        }
    }
    if !folded.is_empty() {
        let under: u128 = folded.iter().map(|c| c.subtree).sum();
        rows.push(Row {
            depth,
            node: format!("{} others", folded.len()),
            own: under,
            subtree: under,
            percent: percent(under, total),
            route: "folded",
            shared: false,
        });
    }
}

fn percent(part: u128, whole: u128) -> f64 {
    match whole {
        0 => 0.0,
        _ => 100.0 * part as f64 / whole as f64,
    }
}
