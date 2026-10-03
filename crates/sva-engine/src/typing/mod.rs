// Concern: gives every node one Ty and the value it lowered to | Non-concern: the per-term judgment (sva-formula), lowering (lower/) | IO: (Instances, Order) -> Ty per node

mod draft;
mod folds;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_formula::filter::Shape;
use sva_formula::{ClosedForm, Codomain, Env, Held, NodeId, Origin, ParamId, Ty, Var, infer};
use sva_samples::Params;

use crate::arguments::{Arguments, Called, Chosen};
use crate::cache::Stored;
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::Instances;
use crate::lower;
use crate::schedule::Order;
use crate::time::Grid;

use draft::{Draft, Entry, Units};

/// A closed form is cast-free on one axis; every crossing is its own node.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    ClosedForm(ClosedForm),
    Cast(Cast, NodeId),
    Op {
        name: String,
        args: Vec<NodeId>,
    },
    SelfAt {
        at: When,
    },
    Read {
        source: NodeId,
        at: When,
        site: Origin,
    },
    Noise(u64),
    /// The signal and its three arguments, each a node of its own.
    Filter {
        shape: Shape,
        x: NodeId,
        cutoff: NodeId,
        q: NodeId,
        gain: NodeId,
    },
    Solver {
        params: Box<Params>,
        varying: Vec<(&'static str, NodeId)>,
    },
    /// Samples the store answered in place of the node's own source, which is never typed.
    Stored(Arc<Stored>),
}

/// A read's instant: the exact time `k*t + s` written; a closed form of `t` held as a node;
/// or an integer each sample evaluates.
#[derive(Clone, Debug, PartialEq)]
pub enum When {
    At(crate::time::Affine),
    Moving(NodeId),
    Index(crate::index::Index),
    Step(Step),
}

/// An integer no one map spells: exact indices, `idx(time, round)` of a time that moves, and
/// sums, negations and products of them.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Index(crate::index::Index),
    Nearest(NodeId, crate::index::Round),
    Add(Vec<Step>),
    Neg(Box<Step>),
    Mul(Vec<Step>),
}

impl Step {
    /// Every time that moves in it.
    pub(crate) fn times(&self, out: &mut Vec<NodeId>) {
        match self {
            Step::Index(_) => {}
            Step::Nearest(time, _) => out.push(*time),
            Step::Add(parts) | Step::Mul(parts) => parts.iter().for_each(|p| p.times(out)),
            Step::Neg(p) => p.times(out),
        }
    }
}

impl When {
    /// Every node it evaluates each sample.
    pub(crate) fn moving(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        match self {
            When::Moving(id) => out.push(*id),
            When::Step(step) => step.times(&mut out),
            When::At(_) | When::Index(_) => {}
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub ty: Ty,
    pub var: Var,
    pub value: Value,
    pub grid: Grid,
}

/// Every node keeps one id while held; a node lowered anew takes a free one.
#[derive(Debug, Default, PartialEq)]
pub struct Typing {
    nodes: Vec<Option<Node>>,
    free: Vec<u32>,
    arguments: BTreeMap<String, Arguments>,
    by_path: BTreeMap<String, NodeId>,
    /// Paths `by_path` holds for a file's one instance.
    aliases: BTreeSet<String>,
    copies: BTreeMap<String, BTreeMap<Grid, NodeId>>,
    lowering: BTreeSet<String>,
    files: BTreeMap<String, Vec<NodeId>>,
    origins: Vec<Option<(u32, Option<sva_ast::ByteSpan>)>>,
    free_origins: Vec<u32>,
    /// Each node name origins mark, and how many.
    sites: Vec<Option<(String, u32)>>,
    free_sites: Vec<u32>,
    site_ids: BTreeMap<String, u32>,
    pending: BTreeSet<NodeId>,
    indices: u32,
    sum: Option<(NodeId, Vec<SumSlot>)>,
    folds: folds::Folds,
    /// Every node the latest draft lowered, in order.
    lowered: Vec<String>,
    units: BTreeMap<String, Units>,
    making: Vec<(String, Grid)>,
    draft: Draft,
}

/// One term of a stream's note sum: its node, or the hull of the supports of those it retired.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SumSlot {
    Node(NodeId),
    Retired(sva_samples::Extent),
}

impl Typing {
    /// `node` named by its terms in place.
    pub(crate) fn name_sum(&mut self, node: NodeId, slots: Vec<SumSlot>) {
        self.draft.sum = Some(Some((node, slots)));
        self.folds.clear();
    }

    fn summed(&self) -> Option<&(NodeId, Vec<SumSlot>)> {
        match &self.draft.sum {
            Some(staged) => staged.as_ref(),
            None => self.sum.as_ref(),
        }
    }

    /// The note sum, where it holds a term it retired.
    pub(crate) fn retired_sum(&self) -> Option<NodeId> {
        let (sum, slots) = self.summed()?;
        let retired = slots.iter().any(|slot| matches!(slot, SumSlot::Retired(_)));
        retired.then_some(*sum)
    }

    /// The edges a node's identity is hashed over.
    pub(crate) fn operands(&self, id: NodeId) -> Vec<NodeId> {
        match self.value(id) {
            Value::ClosedForm(form) => crate::refs::nodes_in(&form.body),
            Value::Cast(_, source) | Value::Read { source, .. } => vec![*source],
            Value::Op { args, .. } => args.clone(),
            Value::Filter {
                x, cutoff, q, gain, ..
            } => vec![*x, *cutoff, *q, *gain],
            Value::Solver { varying, .. } => varying.iter().map(|(_, a)| *a).collect(),
            Value::SelfAt { .. } | Value::Noise(_) | Value::Stored(_) => Vec::new(),
        }
    }

    /// Whether `id` reads `of`, however far down.
    pub(crate) fn reads(&self, id: NodeId, of: NodeId) -> bool {
        let (mut open, mut seen) = (vec![id], BTreeSet::new());
        while let Some(at) = open.pop() {
            if at == of {
                return true;
            }
            if seen.insert(at) {
                open.extend(self.operands(at));
            }
        }
        false
    }

    pub(crate) fn sum_slots(&self, node: NodeId) -> Option<&[SumSlot]> {
        self.summed()
            .filter(|(held, _)| *held == node)
            .map(|(_, slots)| slots.as_slice())
    }

    pub(crate) fn lowering(&mut self, path: &str) {
        self.lowered.push(path.to_string());
    }

    pub(crate) fn lowered(&self) -> &[String] {
        &self.lowered
    }

    pub(crate) fn next_index(&mut self) -> sva_formula::IndexId {
        self.indices += 1;
        sva_formula::IndexId(self.indices)
    }

    /// A call lowered twice is noted once, at its latest lowering.
    pub(crate) fn note(&mut self, node: &str, call: Option<Called>, chosen: Vec<Chosen>) {
        let mut held = self.arguments(node).cloned().unwrap_or_else(|| Arguments {
            node: node.to_string(),
            ..Arguments::default()
        });
        if let Some(call) = call {
            held.calls
                .retain(|c| (c.at.start, &c.name) != (call.at.start, &call.name));
            held.calls.push(call);
        }
        for one in chosen {
            held.chosen.retain(|c| c.at.start != one.at.start);
            held.chosen.push(one);
        }
        let old = self.draft.arguments.insert(node.to_string(), held);
        self.draft.journal.push(Entry::Noted(node.to_string(), old));
    }

    pub fn arguments(&self, node: &str) -> Option<&Arguments> {
        match self.draft.arguments.get(node) {
            Some(held) => Some(held),
            None if self.draft.hidden.contains(node) => None,
            None => self.arguments.get(node),
        }
    }

    pub fn ty(&self, n: NodeId) -> Ty {
        self.at(n).ty
    }

    pub fn var(&self, n: NodeId) -> Var {
        self.at(n).var
    }

    pub fn value(&self, n: NodeId) -> &Value {
        &self.at(n).value
    }

    pub fn name(&self, n: NodeId) -> &str {
        &self.at(n).name
    }

    pub fn grid(&self, n: NodeId) -> Grid {
        self.at(n).grid
    }

    pub(crate) fn copy(&self, path: &str, grid: Grid) -> Option<NodeId> {
        let key = (path.to_string(), grid);
        match self.draft.copies.get(&key) {
            Some(id) => Some(*id),
            None if self.draft.hidden.contains(path) => None,
            None => self
                .copies
                .get(path)
                .and_then(|held| held.get(&grid))
                .copied(),
        }
    }

    pub(crate) fn copied(&mut self, path: &str, grid: Grid, id: NodeId) {
        let key = (path.to_string(), grid);
        let old = self.draft.copies.insert(key.clone(), id);
        self.draft.journal.push(Entry::Copy(key, old));
    }

    /// `false` where a copy of `path` is already being lowered: a loop of refs.
    pub(crate) fn opened(&mut self, path: &str) -> bool {
        self.lowering.insert(path.to_string())
    }

    pub(crate) fn closed(&mut self, path: &str) {
        self.lowering.remove(path);
    }

    pub fn at(&self, n: NodeId) -> &Node {
        self.nodes[n.0 as usize]
            .as_ref()
            .expect("a node the typing holds")
    }

    pub fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        let held = self.nodes.iter().enumerate();
        held.filter(|(_, n)| n.is_some())
            .map(|(at, _)| NodeId(at as u32))
    }

    pub(crate) fn len(&self) -> usize {
        self.nodes.len() - self.free.len()
    }

    pub fn id(&self, path: &str) -> Option<NodeId> {
        match self.draft.by_path.get(path) {
            Some(id) => Some(*id),
            None if self.draft.hidden.contains(path) => None,
            None => self.by_path.get(path).copied(),
        }
    }

    /// Every path named, the draft's over the rest.
    pub fn paths(&self) -> impl Iterator<Item = (&str, NodeId)> {
        let drafted = self.draft.by_path.iter();
        let kept = self.by_path.iter().filter(|(path, _)| {
            !self.draft.hidden.contains(*path) && !self.draft.by_path.contains_key(*path)
        });
        let all: BTreeMap<&String, &NodeId> = drafted.chain(kept).collect();
        all.into_iter().map(|(p, id)| (p.as_str(), *id))
    }

    /// The node a reading asks for, by instance path or by the file a composer wrote.
    pub fn resolve(&self, path: &str) -> Result<NodeId, EngineError> {
        if let Some(id) = self.id(path) {
            return Ok(id);
        }
        let held = self.files.get(path).cloned().unwrap_or_default();
        crate::instantiate::sole(
            path,
            held.into_iter()
                .map(|id| (self.name(id).to_string(), id))
                .collect(),
        )
    }

    /// Every `Origin` a term carries was stamped here.
    pub fn locate(&self, origin: Origin) -> Located {
        let held = self.origins.get(origin.token() as usize).copied().flatten();
        held.and_then(|(site, span)| {
            let (name, _) = self.sites[site as usize].as_ref()?;
            Some(Located::at(name.as_str(), span))
        })
        .unwrap_or_default()
    }

    pub(crate) fn mark(&mut self, node: &str, span: Option<sva_ast::ByteSpan>) -> Origin {
        let site = match self.site_ids.get(node) {
            Some(&site) => site,
            None => {
                let held = Some((node.to_string(), 0));
                let site = match self.free_sites.pop() {
                    Some(site) => {
                        self.sites[site as usize] = held;
                        site
                    }
                    None => {
                        self.sites.push(held);
                        (self.sites.len() - 1) as u32
                    }
                };
                self.site_ids.insert(node.to_string(), site);
                site
            }
        };
        self.sites[site as usize].as_mut().expect("a site").1 += 1;
        let token = match self.free_origins.pop() {
            Some(token) => {
                self.origins[token as usize] = Some((site, span));
                token
            }
            None => {
                self.origins.push(Some((site, span)));
                (self.origins.len() - 1) as u32
            }
        };
        let unit = self.unit();
        self.draft.journal.push(Entry::Origin(token, unit));
        Origin::new(token)
    }

    pub(crate) fn seed(&mut self, path: &str, held: Held, grid: Grid) -> NodeId {
        if let Some(id) = self.id(path) {
            return id;
        }
        self.begin(path, grid);
        let id = self.push(
            Node {
                name: path.to_string(),
                ty: Ty {
                    dual: held.is_closed_form(),
                    ..Ty::discrete(held, Codomain::Real)
                },
                var: Var::T,
                value: Value::Op {
                    name: "loop".to_string(),
                    args: Vec::new(),
                },
                grid,
            },
            Some(path),
        );
        self.end();
        self.pending.insert(id);
        self.draft.journal.push(Entry::Pending(id));
        id
    }

    pub(crate) fn pending(&self, id: NodeId) -> bool {
        self.pending.contains(&id)
    }

    /// Settles a seed this draft made.
    pub(crate) fn settle(&mut self, id: NodeId, node: Node) {
        self.nodes[id.0 as usize] = Some(node);
        self.pending.remove(&id);
        self.folds.clear();
    }

    pub(crate) fn folds(&self) -> &folds::Folds {
        &self.folds
    }

    pub(crate) fn alias(&mut self, path: &str, id: NodeId) {
        let old = self.draft.by_path.insert(path.to_string(), id);
        self.draft.journal.push(Entry::Path(path.to_string(), old));
    }

    pub(crate) fn push(&mut self, node: Node, path: Option<&str>) -> NodeId {
        let id = match self.free.pop() {
            Some(at) => {
                self.nodes[at as usize] = Some(node);
                NodeId(at)
            }
            None => {
                self.nodes.push(Some(node));
                NodeId((self.nodes.len() - 1) as u32)
            }
        };
        let unit = self.unit();
        self.draft.journal.push(Entry::Node(id, unit));
        if let Some(path) = path {
            self.alias(path, id);
        }
        id
    }

    /// What is made until `end` is `path`'s, on `grid`.
    pub(crate) fn begin(&mut self, path: &str, grid: Grid) {
        self.making.push((path.to_string(), grid));
    }

    pub(crate) fn end(&mut self) {
        self.making.pop();
    }

    fn unit(&self) -> (String, Grid) {
        self.making
            .last()
            .cloned()
            .unwrap_or_else(|| (String::new(), Grid::of(1)))
    }

    pub(crate) fn infer_closed_form(&self, form: &ClosedForm) -> Result<Ty, EngineError> {
        let inferred =
            crate::refs::read_through(self, |through| infer(form, &Table(&self.nodes, through)));
        inferred.map_err(|r| {
            EngineError::of_closed_form(
                &r,
                self.locate(r.origin),
                "write the subterm inside sample(...) to leave A deliberately",
            )
        })
    }
}

struct Table<'a>(&'a [Option<Node>], &'a dyn sva_formula::Reads);

impl Env for Table<'_> {
    fn node(&self, id: NodeId) -> Ty {
        self.0[id.0 as usize]
            .as_ref()
            .expect("a node the typing holds")
            .ty
    }

    /// Substituted per instance before a closed form reaches sva-formula, so no term holds one.
    fn param(&self, _: ParamId) -> Ty {
        Ty::form(Var::T, true, Codomain::Real)
    }

    fn reads(&self) -> &dyn sva_formula::Reads {
        self.1
    }
}

/// Dependencies first, so a ref reads a type already decided.
pub fn infer_all(inst: &Instances, order: &Order) -> Result<Typing, EngineError> {
    let mut typing = Typing::default();
    typing.lower(inst, &order.groups)?;
    typing.commit(inst);
    Ok(typing)
}

impl Typing {
    /// Lowers `groups`, dependencies first, beside what is held, each path hidden first.
    pub(crate) fn lower(
        &mut self,
        inst: &Instances,
        groups: &[Vec<String>],
    ) -> Result<(), EngineError> {
        self.lowered.clear();
        self.hide(groups.iter().flatten().cloned());
        for group in groups {
            match crate::schedule::is_loop(inst, group) {
                true => settle_loop(self, inst, group)?,
                false => {
                    for path in group {
                        lower::node(path, inst, self)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Each node `stored` names stands as the samples memory answered it with: what it
    /// computes is what it was, so its readers read it as they would have.
    pub(crate) fn stand(&mut self, stored: &BTreeMap<String, Arc<Stored>>) {
        for (path, held) in stored {
            let Some(id) = self.id(path) else {
                continue;
            };
            let grid = self.grid(id);
            self.nodes[id.0 as usize] = Some(Node {
                grid,
                ..standing(path, held)
            });
        }
        self.folds.clear();
    }
}

fn standing(path: &str, held: &Arc<Stored>) -> Node {
    Node {
        name: path.to_string(),
        ty: Ty {
            width: held.width,
            rate: held.rate,
            ..Ty::discrete(Held::Sampled, held.codomain)
        },
        var: Var::T,
        value: Value::Stored(Arc::clone(held)),
        grid: held.grid,
    }
}

const SEEDS: [Held; 2] = [Held::Form(Var::T), Held::Sampled];

/// A group's members type together, so the seed a pass starts from is a guess the pass
/// either reaches again or replaces with what its members held.
fn settle_loop(typing: &mut Typing, inst: &Instances, group: &[String]) -> Result<(), EngineError> {
    let mut seed = SEEDS[0];
    let mut refusal = None;
    for _ in 0..=SEEDS.len() {
        let mark = typing.checkpoint();
        for path in group {
            typing.seed(path, seed, inst.grid());
        }
        let walked = group
            .iter()
            .try_for_each(|path| lower::node(path, inst, typing).map(|_| ()));
        match walked {
            Ok(()) => match held_by(typing, group) {
                reached if reached == seed => return Ok(()),
                reached => seed = reached,
            },
            Err(e) => {
                refusal = Some(e);
                seed = elsewhere(seed);
            }
        }
        typing.rollback(mark);
    }
    Err(refusal.unwrap_or_else(|| mixed(group)))
}

/// The seed a refused pass did not try.
fn elsewhere(seed: Held) -> Held {
    match seed {
        Held::Sampled => Held::Form(Var::T),
        _ => Held::Sampled,
    }
}

/// One representation for the whole group: samples where any member reached them.
fn held_by(typing: &Typing, group: &[String]) -> Held {
    let sampled = group
        .iter()
        .filter_map(|path| typing.id(path))
        .any(|id| !typing.ty(id).is_closed_form());
    match sampled {
        true => Held::Sampled,
        false => Held::Form(Var::T),
    }
}

/// A group that settles in no representation holds both.
fn mixed(group: &[String]) -> EngineError {
    let at = group.first().map_or("", String::as_str);
    EngineError::refused(Diagnostic {
        code: "type.samples_in_closed_form".to_string(),
        message: format!(
            "the loop over `{}` holds a closed form and samples at once.",
            group.join("`, `")
        ),
        location: Located::at(at, None),
        help: "write sample(...) on the members that are closed forms, so the whole loop runs \
               on the grid"
            .to_string(),
    })
}
