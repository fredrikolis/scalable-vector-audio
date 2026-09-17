// Concern: gives every node across the graph one Ty and the value it lowered to | Non-concern: the per-term judgment (sva-formula), lowering one (lower/) | IO: (Instances, Order) -> Ty per node

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::filter::Shape;
use sva_formula::{ClosedForm, Codomain, Env, Held, NodeId, Origin, ParamId, Ty, Var, infer};
use sva_samples::Params;

use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::Instances;
use crate::loops::Delay;
use crate::lower;
use crate::offset::Offset;
use crate::schedule::Order;

/// A closed form is cast-free on one axis; every crossing is its own node.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    ClosedForm(ClosedForm),
    Cast(Cast, NodeId),
    Op {
        name: String,
        args: Vec<NodeId>,
    },
    /// One read of this node's own output, at the delay the call site wrote.
    SelfAt(Delay),
    /// A count of samples read as a duration, which only a rate turns into seconds.
    Grid(f64),
    Read {
        source: NodeId,
        at: Offset,
        site: Origin,
    },
    /// The signal and its three arguments, each a node of its own.
    Filter {
        shape: Shape,
        x: NodeId,
        cutoff: NodeId,
        q: NodeId,
        gain: NodeId,
    },
    Solver(Box<Params>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub ty: Ty,
    pub var: Var,
    pub value: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Typing {
    nodes: Vec<Node>,
    by_path: BTreeMap<String, NodeId>,
    files: BTreeMap<String, Vec<NodeId>>,
    origins: Vec<Located>,
    pending: BTreeSet<NodeId>,
    indices: u32,
}

impl Typing {
    pub(crate) fn next_index(&mut self) -> sva_formula::IndexId {
        self.indices += 1;
        sva_formula::IndexId(self.indices)
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

    pub fn at(&self, n: NodeId) -> &Node {
        &self.nodes[n.0 as usize]
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
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn mark(&mut self, at: Located) -> Origin {
        self.origins.push(at);
        Origin::new((self.origins.len() - 1) as u32)
    }

    pub(crate) fn seed(&mut self, path: &str, held: Held) -> NodeId {
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
    let mut typing = Typing::default();
    for group in &order.groups {
        match order.is_loop(group) {
            true => settle_loop(&mut typing, inst, group)?,
            false => {
                for path in group {
                    lower::node(path, inst, &mut typing)?;
                }
            }
        }
    }
    name_files(&mut typing, inst);
    Ok(typing)
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
            attempt.seed(path, seed);
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
