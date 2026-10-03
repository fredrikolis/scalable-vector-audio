// Concern: what reading another node yields, per the representation it holds | Non-concern: ordering the reads (schedule.rs), collapsing a closed form (sva-samples) | IO: (NodeId, Var) -> SpectralSum

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;

use sva_formula::closed_form::children;
use sva_formula::spectral_sum::atom::Indicator;
use sva_formula::spectral_sum::build::{multiply_lanes_read, sole_constant};
use sva_formula::spectral_sum::image;
use sva_formula::spectral_sum::merge::simplify;
use sva_formula::{
    Body, C64, ClosedForm, Lane, Left, NodeId, Opaque, Part, Reads, SpectralSum, Through, Var,
    dual_read, inverse_read, normalize_closed_form, normalize_read,
};

use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::typing::{Typing, Value};

mod identity;
mod prefix;

pub use identity::{identity, symbolic_hash};
pub(crate) use identity::{passes, subterm_identity};
pub(crate) use prefix::switches;

/// The spectral sum of one node read on `want`'s axis, with every ref it holds already
/// composed in. A pair answers on either axis; anything else answers on its own.
pub fn spectral_sum_of(
    typing: &Typing,
    node: NodeId,
    want: Var,
) -> Result<SpectralSum, EngineError> {
    let held = |id: NodeId| {
        let var = typing.var(id);
        let key = identity(typing, id).ok();
        typing.folds().refused((id, var)).is_some()
            || key.is_some_and(|key| typing.folds().composed((key, var)).is_some())
    };
    for read in typing.unfolded_over(node, |id| composes(typing, id), held) {
        if read != node {
            let _ = composed(typing, read, typing.var(read), &mut Open::default());
        }
    }
    composed(typing, node, want, &mut Open::default())
}

fn composes(typing: &Typing, id: NodeId) -> Vec<NodeId> {
    match typing.value(id) {
        Value::ClosedForm(form) => nodes_in(&form.body),
        Value::Cast(Cast::Fourier | Cast::IFourier, source) => vec![*source],
        _ => Vec::new(),
    }
}

/// Whether a composition met a ref still being composed: that refusal depends on where the
/// walk entered the loop.
#[derive(Default)]
struct Open {
    chain: Vec<NodeId>,
    cut: bool,
}

/// Kept under the node's identity, unlocated, and answered as written where `node` is: two
/// nodes that are one value compose once. A refusal is kept per node, unless a loop of refs
/// made it.
fn composed(
    typing: &Typing,
    node: NodeId,
    want: Var,
    open: &mut Open,
) -> Result<SpectralSum, EngineError> {
    let here = written_at(typing, node);
    let key = identity(typing, node).ok().map(|held| (held, want));
    if let Some(held) = key.and_then(|key| typing.folds().composed(key)) {
        return Ok(held.located(here));
    }
    if let Some(refused) = typing.folds().refused((node, want)) {
        return Err(refused);
    }
    let outer = std::mem::take(&mut open.cut);
    let found = composing(typing, node, want, open);
    match (&found, key) {
        (Ok(found), Some(key)) => {
            let unlocated = found.clone().located(sva_formula::Origin::UNKNOWN);
            typing.folds().keep_composed(key, unlocated);
        }
        (Err(refused), _) if !open.cut => {
            typing.folds().keep_refused((node, want), refused.clone());
        }
        _ => {}
    }
    open.cut |= outer;
    Ok(found?.located(here))
}

fn written_at(typing: &Typing, node: NodeId) -> sva_formula::Origin {
    match typing.value(node) {
        Value::ClosedForm(form) => form.origin,
        Value::Cast(_, source) => written_at(typing, *source),
        _ => sva_formula::Origin::UNKNOWN,
    }
}

/// A form reaching itself through a ref `open` still composes is a loop no substitution
/// closes.
fn composing(
    typing: &Typing,
    node: NodeId,
    want: Var,
    open: &mut Open,
) -> Result<SpectralSum, EngineError> {
    #[cfg(test)]
    typing
        .folds()
        .composings
        .set(typing.folds().composings.get() + 1);
    if open.chain.contains(&node) {
        open.cut = true;
        return Err(cyclic(typing, node));
    }
    open.chain.push(node);
    let held = match typing.value(node) {
        Value::ClosedForm(form) => {
            let body = fold_constants(typing, &form.body);
            compose(typing, &body, form.var, open)?
        }
        Value::Cast(Cast::Fourier, source) => {
            let inner = composed(typing, *source, Var::T, open)?;
            turn(typing, node, read_through(typing, |t| dual_read(&inner, t)))?
        }
        Value::Cast(Cast::IFourier, source) => {
            let inner = composed(typing, *source, Var::F, open)?;
            turn(
                typing,
                node,
                read_through(typing, |t| inverse_read(&inner, t)),
            )?
        }
        Value::Op { name, .. } if typing.ty(node).is_closed_form() => {
            return Err(across(typing, node, name));
        }
        _ => return Err(no_closed_form(typing, node)),
    };
    open.chain.pop();
    on_axis(typing, node, held, typing.var(node), want)
}

pub(crate) fn cyclic(typing: &Typing, node: NodeId) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.cyclic_substitution".to_string(),
        message: format!(
            "`{}` reads itself around a loop of refs.",
            typing.name(node)
        ),
        location: Located::at(typing.name(node), None),
        help: "write the loop with self(...), which the engine classifies".to_string(),
    })
}

fn on_axis(
    typing: &Typing,
    node: NodeId,
    held: SpectralSum,
    axis: Var,
    want: Var,
) -> Result<SpectralSum, EngineError> {
    if axis == want {
        return Ok(held);
    }
    let turned = read_through(typing, |through| match want {
        Var::F => dual_read(&held, through),
        Var::T => inverse_read(&held, through),
    });
    turn(typing, node, turned)
}

fn turn(
    typing: &Typing,
    node: NodeId,
    turned: Result<SpectralSum, Left>,
) -> Result<SpectralSum, EngineError> {
    turned.map_err(|left| {
        EngineError::of_closed_form(
            &left.refusal(),
            typing.locate(left.origin),
            format!(
                "write `{}` inside sample(...) to leave A deliberately",
                typing.name(node)
            ),
        )
    })
}

/// A form whose operands crossed a cast is held as an operation over values, and only
/// the written form itself has atoms to compose.
pub(crate) fn across(typing: &Typing, node: NodeId, call: &str) -> EngineError {
    no_spectral_sum(typing.name(node), call)
}

/// An exact reading answers off a spectral sum, so a term that reaches none says which
/// subterm blocked it rather than which reading asked.
fn no_spectral_sum(node: &str, blocking: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "read.no_spectral_sum".to_string(),
        message: format!(
            "`{node}` has no spectral sum to read: `{blocking}` composes no value across a ref."
        ),
        location: Located::at(node, None),
        help: "write the construct inside the node it reads, or read it off sample(...)"
            .to_string(),
    })
}

fn no_closed_form(typing: &Typing, node: NodeId) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "type.samples_in_closed_form".to_string(),
        message: format!(
            "`{}` is samples; nothing returns from samples to a closed form",
            typing.name(node)
        ),
        location: Located::at(typing.name(node), None),
        help: "read it as samples, or build the closed form without it".to_string(),
    })
}

/// Every ref naming one number, replaced by that number. A constant is the same value on
/// either axis and under every construct, so it folds where no form would substitute.
pub(crate) fn fold_constants<'a>(typing: &Typing, f: &'a Body) -> Cow<'a, Body> {
    let fold = &mut Folding::default();
    match names_number(typing, f, fold) {
        true => Cow::Owned(folded(typing, f, fold)),
        false => Cow::Borrowed(f),
    }
}

fn names_number(typing: &Typing, f: &Body, fold: &mut Folding) -> bool {
    match f {
        Body::Node(id) => number(typing, *id, fold).is_some(),
        _ => children(f)
            .into_iter()
            .any(|p| names_number(typing, &p.body, fold)),
    }
}

/// The chain of refs still being folded, and whether a fold met one of them again: a number
/// that loop cut short is not the node's own, so it is not kept.
#[derive(Default)]
struct Folding {
    open: Vec<NodeId>,
    cut: bool,
}

fn folded(typing: &Typing, f: &Body, fold: &mut Folding) -> Body {
    let Body::Node(id) = f else {
        return sva_formula::closed_form::map_children(f, |p| {
            Part::new(p.origin, folded(typing, &p.body, fold))
        });
    };
    match number(typing, *id, fold) {
        Some(c) => Body::Const(c),
        None => f.clone(),
    }
}

/// The one number a node holds, or `None` where it holds a form, samples or a ref loop.
fn number(typing: &Typing, node: NodeId, fold: &mut Folding) -> Option<C64> {
    if let Some(held) = typing.folds().number(node) {
        return held;
    }
    if fold.open.contains(&node) {
        fold.cut = true;
        return None;
    }
    let Value::ClosedForm(form) = typing.value(node) else {
        return None;
    };
    if let Body::Const(c) = form.body {
        return Some(c);
    }
    let outer = std::mem::take(&mut fold.cut);
    fold.open.push(node);
    let folded = names_number(typing, &form.body, fold).then(|| ClosedForm {
        body: folded(typing, &form.body, fold),
        ..*form
    });
    fold.open.pop();
    let number = normalize_closed_form(folded.as_ref().unwrap_or(form))
        .ok()
        .and_then(|sum| sole_constant(&sum));
    if !fold.cut {
        typing.folds().keep_number(node, number);
    }
    fold.cut |= outer;
    number
}

/// A node reference is not a `Body`, so a term holding one is normalized by composing
/// the pieces around it rather than by handing the whole tree to `normalize`.
fn compose(
    typing: &Typing,
    body: &Body,
    var: Var,
    open: &mut Open,
) -> Result<SpectralSum, EngineError> {
    let here = *open
        .chain
        .last()
        .expect("compose runs inside the node it composes");
    if !holds_node(body) {
        return normalize_here(typing, body, var);
    }
    match body {
        Body::Node(id) => composed(typing, *id, var, open),
        Body::Add(parts) => {
            let mut lanes: Vec<Lane> = Vec::new();
            for part in parts {
                add_into(&mut lanes, compose(typing, &part.body, var, open)?);
            }
            Ok(sum(var, lanes))
        }
        Body::Mul(parts) => {
            let mut acc: Option<SpectralSum> = None;
            for part in parts {
                let next = compose(typing, &part.body, var, open)?;
                acc = Some(match acc {
                    None => next,
                    Some(held) => multiply(typing, &held, &next, var)?,
                });
            }
            Ok(acc.unwrap_or_else(|| sum(var, Vec::new())))
        }
        Body::Shift { by, of } => {
            let held = compose(typing, &of.body, var, open)?;
            image::shift(held, *by).map_err(|left| left_of(typing, left))
        }
        Body::Crop {
            of,
            l,
            r,
            rise,
            fall,
        } if *rise > 0.0 || *fall > 0.0 => {
            let held = compose(typing, &of.body, var, open)?;
            let window = image::crop_window(*l, *r, *rise, *fall, of.origin, var);
            multiply(typing, &held, &window, var)
        }
        Body::Crop { of, l, r, .. } => {
            let held = compose(typing, &of.body, var, open)?;
            image::crop(held, Indicator { l: *l, r: *r }).map_err(|left| left_of(typing, left))
        }
        Body::Div(num, den) => {
            let over = compose(typing, &den.body, var, open)?;
            let numerator = compose(typing, &num.body, var, open)?;
            multiply(typing, &numerator, &reciprocal(typing, here, &over)?, var)
        }
        Body::Join(parts) => {
            let mut lanes = Vec::new();
            for part in parts {
                lanes.extend(compose(typing, &part.body, var, open)?.lanes);
            }
            Ok(sum(var, lanes))
        }
        Body::Channel(of, k) => {
            let held = compose(typing, &of.body, var, open)?;
            match held.lanes.into_iter().nth(usize::from(*k)) {
                Some(lane) => Ok(sum(var, vec![lane])),
                None => Err(unsubstituted(typing, here, body)),
            }
        }
        // Every other construct is what it was written as, each ref read as its own form.
        other if reads_through(typing, other, var) => read_through(typing, |through| {
            normalize_with(typing, other, var, through)
        }),
        other => Err(unsubstituted(typing, here, other)),
    }
}

/// A divisor a ref reaches has to be one number: a reciprocal is not an atom sum.
fn reciprocal(
    typing: &Typing,
    node: NodeId,
    over: &SpectralSum,
) -> Result<SpectralSum, EngineError> {
    let divided = || no_spectral_sum(typing.name(node), "a division by a closed form");
    let [lane] = over.lanes.as_slice() else {
        return Err(divided());
    };
    match lane.atoms.as_slice() {
        [atom] if atom.is_bare() => Ok(SpectralSum::mono(
            over.var,
            vec![sva_formula::spectral_sum::atom::SpectralAtom::constant(
                atom.c.inv(),
                atom.origin,
            )],
        )),
        _ => Err(divided()),
    }
}

fn left_of(typing: &Typing, left: Left) -> EngineError {
    EngineError::of_closed_form(
        &left.refusal(),
        typing.locate(left.origin),
        "write the subterm inside sample(...) to leave A deliberately",
    )
}

fn normalize_here(typing: &Typing, body: &Body, var: Var) -> Result<SpectralSum, EngineError> {
    normalize_with(typing, body, var, &Opaque)
}

fn normalize_with(
    typing: &Typing,
    body: &Body,
    var: Var,
    reads: &dyn Reads,
) -> Result<SpectralSum, EngineError> {
    normalize_read(body, var, reads).map_err(|left| {
        EngineError::of_closed_form(
            &left.refusal(),
            typing.locate(left.origin),
            "write the subterm inside sample(...) to leave A deliberately",
        )
    })
}

/// A ref reaching a value no substitution inlines, under a construct with no lane rule of
/// its own: the reading has a name and nothing to read it off.
fn unsubstituted(typing: &Typing, node: NodeId, body: &Body) -> EngineError {
    no_spectral_sum(typing.name(node), named(body))
}

fn named(body: &Body) -> &'static str {
    match body {
        Body::Apply(op, _) => op.name(),
        Body::Pow(..) => "pow",
        Body::Fold(..) => "max, min or mod",
        Body::Join(_) => "join",
        Body::Channel(..) => "ch",
        Body::Series(_) => "sum",
        Body::Delta { .. } => "delta",
        Body::Pv(_) => "pv",
        Body::Deriv { .. } => "a derivative",
        Body::Warp { .. } => "a warped time",
        _ => "a construct",
    }
}

/// Every node a written form names, in written order, so a caller answers each one.
pub fn nodes_in(f: &Body) -> Vec<NodeId> {
    let mut out = Vec::new();
    collect_nodes(f, &mut out);
    out
}

fn collect_nodes(f: &Body, out: &mut Vec<NodeId>) {
    if let Body::Node(id) = f {
        if !out.contains(id) {
            out.push(*id);
        }
        return;
    }
    for part in children(f) {
        collect_nodes(&part.body, out);
    }
}

fn holds_node(f: &Body) -> bool {
    matches!(f, Body::Node(_)) || children(f).iter().any(|p| holds_node(&p.body))
}

fn sum(var: Var, mut lanes: Vec<Lane>) -> SpectralSum {
    for lane in &mut lanes {
        simplify(lane);
    }
    SpectralSum::of(var, lanes)
}

/// A width-1 operand broadcasts into every lane of the wider one, at the operator.
fn add_into(lanes: &mut Vec<Lane>, other: SpectralSum) {
    if other.lanes.is_empty() {
        return;
    }
    let width = lanes.len().max(other.lanes.len());
    if lanes.len() == 1 {
        let held = lanes[0].clone();
        lanes.resize(width, held);
    }
    for at in 0..width {
        let lane = lane_at(&other, at).clone();
        match lanes.get_mut(at) {
            Some(held) => {
                held.atoms.extend(lane.atoms);
                held.series.extend(lane.series);
                held.modal.extend(lane.modal);
            }
            None => lanes.push(lane),
        }
    }
}

fn multiply(
    typing: &Typing,
    a: &SpectralSum,
    b: &SpectralSum,
    var: Var,
) -> Result<SpectralSum, EngineError> {
    let width = a.lanes.len().max(b.lanes.len());
    let mut lanes = Vec::with_capacity(width);
    for at in 0..width {
        let (x, y) = (lane_at(a, at).clone(), lane_at(b, at).clone());
        let held = read_through(typing, |through| multiply_lanes_read(x, y, through));
        lanes.push(held.map_err(|left| {
            EngineError::of_closed_form(
                &left.refusal(),
                typing.locate(left.origin),
                "write one of the factors inside sample(...)",
            )
        })?);
    }
    Ok(sum(var, lanes))
}

fn lane_at(n: &SpectralSum, at: usize) -> &Lane {
    n.lanes.get(at).unwrap_or(&n.lanes[0])
}

/// Whether each ref `body` holds names a form on `var`, as do the refs each of those holds,
/// with no loop among them: only then is a ref read as the form it names.
pub(crate) fn reads_through(typing: &Typing, body: &Body, var: Var) -> bool {
    let open = &mut Vec::new();
    nodes_in(body)
        .into_iter()
        .all(|id| inlinable(typing, id, var, open))
}

/// A form that is one series without end whose term reads refs at a time its index moves, as
/// a closed loop's does, each ref read through and with a sum of its own: no sample of it sums
/// those refs read as values, so it is read as one sum.
pub(crate) fn sums_through(typing: &Typing, form: &ClosedForm) -> bool {
    let Body::Series(s) = &form.body else {
        return false;
    };
    let refs = nodes_in(&form.body);
    form.var == Var::T
        && s.hi == sva_formula::Bound::Infinite
        && moves_a_ref(&s.term.body, s.index)
        && reads_through(typing, &form.body, Var::T)
        && refs
            .iter()
            .all(|id| spectral_sum_of(typing, *id, Var::T).is_ok())
}

fn moves_a_ref(f: &Body, k: sva_formula::IndexId) -> bool {
    match f {
        Body::Warp { at, of } if matches!(*of.body, Body::Node(_)) => {
            sva_formula::series::mentions(&at.body, k)
        }
        other => children(other).iter().any(|p| moves_a_ref(&p.body, k)),
    }
}

/// A loop met is a loop the node is on or reaches, wherever the walk entered it.
fn inlinable(typing: &Typing, id: NodeId, var: Var, open: &mut Vec<NodeId>) -> bool {
    if let Some(held) = typing.folds().inlinable((id, var)) {
        return held;
    }
    if open.contains(&id) {
        return false;
    }
    let found = match typing.value(id) {
        Value::ClosedForm(form) if form.var == var => {
            open.push(id);
            let held = nodes_in(&form.body)
                .into_iter()
                .all(|n| inlinable(typing, n, var, open));
            open.pop();
            held
        }
        _ => false,
    };
    typing.folds().keep_inlinable((id, var), found);
    found
}

/// One reading of written forms, found once per node for as long as it lasts.
pub(crate) struct PerNode<T>(RefCell<BTreeMap<NodeId, T>>);

impl<T: Clone> PerNode<T> {
    pub(crate) fn new() -> PerNode<T> {
        PerNode(RefCell::default())
    }

    pub(crate) fn of(&self, id: NodeId, read: impl FnOnce() -> T) -> T {
        if let Some(held) = self.0.borrow().get(&id) {
            return held.clone();
        }
        let found = read();
        self.0.borrow_mut().insert(id, found.clone());
        found
    }

    /// The same, keeping only a reading that did not refuse.
    pub(crate) fn try_of<E>(
        &self,
        id: NodeId,
        read: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        if let Some(held) = self.0.borrow().get(&id) {
            return Ok(held.clone());
        }
        let found = read()?;
        self.0.borrow_mut().insert(id, found.clone());
        Ok(found)
    }
}

/// The written form a ref `reads_through` passed names.
pub(crate) fn written_form(typing: &Typing, id: NodeId) -> &Body {
    match typing.value(id) {
        Value::ClosedForm(form) => &form.body,
        _ => unreachable!("a ref read through names a closed form"),
    }
}

/// `with` handed every ref as the form it names, each node's readings kept on the typing.
pub(crate) fn read_through<R>(typing: &Typing, with: impl FnOnce(&Through) -> R) -> R {
    let written = |id: NodeId| match typing.value(id) {
        Value::ClosedForm(form) => Some((&form.body, form.origin)),
        _ => None,
    };
    with(&Through::new(&written, typing.folds().written()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(name: &str, files: &[(&str, String)], root: &str) -> Typing {
        let dir = std::env::temp_dir().join(format!("sva-refs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        for (file, body) in files {
            std::fs::write(dir.join(file), body).expect("a node file");
        }
        let graph = sva_ast::parse_composition(&dir).expect("a composition");
        crate::types(&graph, root).expect("typed")
    }

    fn composings(tys: &Typing) -> usize {
        tys.folds().composings.get()
    }

    /// Each level reads the one below three times: one composition per node, however many
    /// paths reach it, and none on asking again.
    #[test]
    fn a_chain_composes_each_node_once() {
        let names: Vec<String> = (0..=8).map(|k| format!("n{k}")).collect();
        let mut files = vec![(names[0].as_str(), "sin(2*pi*220*t)\n".to_string())];
        for pair in names.windows(2) {
            let p = format!("@{}(t)", pair[0]);
            files.push((&pair[1], format!("{p}*0.5 + {p}*0.3 + {p}*0.2\n")));
        }
        let tys = typed("chain", &files, "n8");
        let root = tys.id("n8").expect("the root");
        spectral_sum_of(&tys, root, Var::T).expect("a sum");
        assert_eq!(composings(&tys), 9);
        spectral_sum_of(&tys, root, Var::T).expect("a sum");
        assert_eq!(composings(&tys), 9);
    }

    /// Twenty filters deep, the poles pass what a sum holds and every level above refuses:
    /// each refusal is kept, so asking any level again composes nothing.
    #[test]
    fn a_refusal_a_chain_meets_is_composed_once_per_node() {
        let names: Vec<String> = (0..=20).map(|k| format!("n{k}")).collect();
        let tone = "crop(sin(2*pi*220*t), 0s, 0.1s)\n".to_string();
        let mut files = vec![(names[0].as_str(), tone)];
        for pair in names.windows(2) {
            files.push((&pair[1], format!("lowpass(@{}(t), 1000)\n", pair[0])));
        }
        let tys = typed("refused", &files, "n20");
        let at = |k: usize| tys.id(&names[k]).expect("a level");
        assert!(
            spectral_sum_of(&tys, at(20), Var::T).is_err(),
            "the poles refuse"
        );
        let once = composings(&tys);
        for k in (0..=20).rev() {
            let _ = spectral_sum_of(&tys, at(k), Var::T);
        }
        assert_eq!(composings(&tys), once, "a kept refusal composes nothing");
    }

    /// Lowering one node and its reader anew lets go of what they folded to, and only that:
    /// a node beside them composes from what it kept.
    #[test]
    fn an_edit_lets_go_of_the_folds_of_what_it_changed_alone() {
        let dir = std::env::temp_dir().join(format!("sva-refs-narrow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        let files = [
            ("a", "sin(2*pi*220*t)*0.5\n"),
            ("b", "sin(2*pi*330*t)*0.5\n"),
            ("mix", "@a(t) + @b(t)\n"),
        ];
        for (file, body) in files {
            std::fs::write(dir.join(file), body).expect("a node file");
        }
        let graph = sva_ast::parse_composition(&dir).expect("a composition");
        let held = crate::render::prepared(&graph, "mix", 8_000).expect("typed");
        let (inst, mut tys) = (held.instances, held.tys);
        let a = tys.id("a").expect("a");
        spectral_sum_of(&tys, a, Var::T).expect("a sum");
        let before = composings(&tys);
        let changed = [vec!["b".to_string()], vec!["mix".to_string()]];
        tys.lower(&inst, &changed).expect("lowered anew");
        tys.commit(&inst);
        spectral_sum_of(&tys, a, Var::T).expect("a sum");
        assert_eq!(composings(&tys), before, "`a` composes from what it kept");
        let mix = tys.id("mix").expect("mix");
        spectral_sum_of(&tys, mix, Var::T).expect("a sum");
        assert!(composings(&tys) > before, "what changed composes anew");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two differently named nodes holding one body are one identity, so one composition.
    #[test]
    fn two_names_for_one_body_compose_once() {
        let files = [
            ("tone", "sin(2*pi*3*t)\n".to_string()),
            ("a", "@tone(t)*0.5\n".to_string()),
            ("b", "@tone(t)*0.5\n".to_string()),
            ("mix", "@a(t) + @b(t)\n".to_string()),
        ];
        let tys = typed("twins", &files, "mix");
        let [a, b] = ["a", "b"].map(|n| tys.id(n).expect("a node"));
        assert_ne!(a, b);
        assert_eq!(identity(&tys, a).ok(), identity(&tys, b).ok());
        spectral_sum_of(&tys, tys.id("mix").expect("mix"), Var::T).expect("a sum");
        assert_eq!(composings(&tys), 3);
    }
}
