// Concern: gives every node one Ty and the value it lowered to | Non-concern: the per-term judgment (sva-formula), lowering (lower/) | IO: (Instances, Order) -> Ty per node

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_formula::filter::Shape;
use sva_formula::{C64, ClosedForm, Codomain, Env, Held, NodeId, Origin, ParamId, Ty, Var, infer};
use sva_samples::Params;

use crate::arguments::{Arguments, Called, Chosen};
use crate::cache::Stored;
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::Instances;
use crate::lower;
use crate::schedule::Order;
use crate::time::Grid;

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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Typing {
    nodes: Vec<Node>,
    arguments: BTreeMap<String, Arguments>,
    by_path: BTreeMap<String, NodeId>,
    copies: BTreeMap<(String, Grid), NodeId>,
    lowering: BTreeSet<String>,
    files: BTreeMap<String, Vec<NodeId>>,
    origins: Vec<(u32, Option<sva_ast::ByteSpan>)>,
    sites: Vec<String>,
    site_ids: BTreeMap<String, u32>,
    pending: BTreeSet<NodeId>,
    indices: u32,
    sum: Option<(NodeId, Vec<SumSlot>)>,
    numbers: Numbers,
    /// Every node lowered from its source, in order.
    lowered: Vec<String>,
}

/// Each node's folded number, derived from the nodes alone; a settle evicts it all.
#[derive(Clone, Debug, Default)]
struct Numbers(RefCell<BTreeMap<NodeId, Option<C64>>>);

impl PartialEq for Numbers {
    fn eq(&self, _: &Numbers) -> bool {
        true
    }
}

/// One term of a stream's note sum: its node, or the identity and support it had before it
/// ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SumSlot {
    Node(NodeId),
    Retired(sva_formula::Hash, sva_samples::Extent),
}

impl Typing {
    /// `node` named by its terms in place, an ended one by the identity it had.
    pub(crate) fn name_sum(&mut self, node: NodeId, slots: Vec<SumSlot>) {
        self.sum = Some((node, slots));
    }

    pub(crate) fn sum_slots(&self, node: NodeId) -> Option<&[SumSlot]> {
        self.sum
            .as_ref()
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
        let held = self
            .arguments
            .entry(node.to_string())
            .or_insert_with(|| Arguments {
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
    }

    pub fn arguments(&self, node: &str) -> Option<&Arguments> {
        self.arguments.get(node)
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
        self.copies.get(&(path.to_string(), grid)).copied()
    }

    pub(crate) fn copied(&mut self, path: &str, grid: Grid, id: NodeId) {
        self.copies.insert((path.to_string(), grid), id);
    }

    /// `false` where a copy of `path` is already being lowered: a loop of refs.
    pub(crate) fn opened(&mut self, path: &str) -> bool {
        self.lowering.insert(path.to_string())
    }

    pub(crate) fn closed(&mut self, path: &str) {
        self.lowering.remove(path);
    }

    pub fn at(&self, n: NodeId) -> &Node {
        &self.nodes[n.0 as usize]
    }

    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn id(&self, path: &str) -> Option<NodeId> {
        self.by_path.get(path).copied()
    }

    pub fn paths(&self) -> impl Iterator<Item = (&str, NodeId)> {
        self.by_path.iter().map(|(p, id)| (p.as_str(), *id))
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
        self.origins
            .get(origin.token() as usize)
            .map(|&(site, span)| Located::at(self.sites[site as usize].as_str(), span))
            .unwrap_or_default()
    }

    pub(crate) fn mark(&mut self, node: &str, span: Option<sva_ast::ByteSpan>) -> Origin {
        let site = match self.site_ids.get(node) {
            Some(&site) => site,
            None => {
                let site = self.sites.len() as u32;
                self.sites.push(node.to_string());
                self.site_ids.insert(node.to_string(), site);
                site
            }
        };
        self.origins.push((site, span));
        Origin::new((self.origins.len() - 1) as u32)
    }

    pub(crate) fn seed(&mut self, path: &str, held: Held, grid: Grid) -> NodeId {
        if let Some(id) = self.id(path) {
            return id;
        }
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
        self.pending.insert(id);
        id
    }

    pub(crate) fn pending(&self, id: NodeId) -> bool {
        self.pending.contains(&id)
    }

    pub(crate) fn settle(&mut self, id: NodeId, node: Node) {
        self.nodes[id.0 as usize] = node;
        self.pending.remove(&id);
        self.numbers.0.get_mut().clear();
    }

    pub(crate) fn folded_number(&self, id: NodeId) -> Option<Option<C64>> {
        self.numbers.0.borrow().get(&id).copied()
    }

    pub(crate) fn fold_number(&self, id: NodeId, number: Option<C64>) {
        self.numbers.0.borrow_mut().insert(id, number);
    }

    pub(crate) fn alias(&mut self, path: &str, id: NodeId) {
        self.by_path.insert(path.to_string(), id);
    }

    pub(crate) fn push(&mut self, node: Node, path: Option<&str>) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(node);
        if let Some(path) = path {
            self.by_path.insert(path.to_string(), id);
        }
        id
    }

    pub(crate) fn infer_closed_form(&self, form: &ClosedForm) -> Result<Ty, EngineError> {
        infer(form, &Table(&self.nodes)).map_err(|r| {
            EngineError::of_closed_form(
                &r,
                self.locate(r.origin),
                "write the subterm inside sample(...) to leave A deliberately",
            )
        })
    }
}

struct Table<'a>(&'a [Node]);

impl Env for Table<'_> {
    fn node(&self, id: NodeId) -> Ty {
        self.0[id.0 as usize].ty
    }

    /// Substituted per instance before a closed form reaches sva-formula, so no term holds one.
    fn param(&self, _: ParamId) -> Ty {
        Ty::form(Var::T, true, Codomain::Real)
    }
}

/// Dependencies first, so a ref reads a type already decided.
pub fn infer_all(inst: &Instances, order: &Order) -> Result<Typing, EngineError> {
    infer_over(inst, order, &BTreeMap::new())
}

/// Each node `stored` names stands as its samples, and nothing under it is typed.
pub(crate) fn infer_over(
    inst: &Instances,
    order: &Order,
    stored: &BTreeMap<String, Arc<Stored>>,
) -> Result<Typing, EngineError> {
    let mut typing = Typing::default();
    for group in &order.groups {
        match (order.is_loop(group), group.as_slice()) {
            (false, [path]) if let Some(held) = stored.get(path) => {
                typing.push(standing(path, held), Some(path));
            }
            (true, _) => settle_loop(&mut typing, inst, group)?,
            (false, _) => {
                for path in group {
                    lower::node(path, inst, &mut typing)?;
                }
            }
        }
    }
    name_files(&mut typing, inst);
    Ok(typing)
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
        let mut attempt = typing.clone();
        for path in group {
            attempt.seed(path, seed, inst.grid());
        }
        let walked = group
            .iter()
            .try_for_each(|path| lower::node(path, inst, &mut attempt).map(|_| ()));
        match walked {
            Ok(()) => match held_by(&attempt, group) {
                reached if reached == seed => {
                    *typing = attempt;
                    return Ok(());
                }
                reached => seed = reached,
            },
            Err(e) => {
                refusal = Some(e);
                seed = elsewhere(seed);
            }
        }
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

/// A file with one instance answers to its own name too.
fn name_files(typing: &mut Typing, inst: &Instances) {
    let files: Vec<String> = inst
        .paths()
        .filter_map(|p| inst.origin(p))
        .map(str::to_string)
        .collect();
    for file in files {
        if typing.files.contains_key(&file) {
            continue;
        }
        let held: Vec<NodeId> = inst
            .instances_of(&file)
            .filter_map(|p| typing.id(&p))
            .collect();
        if let ([only], None) = (held.as_slice(), typing.id(&file)) {
            typing.alias(&file, *only);
        }
        typing.files.insert(file, held);
    }
}
