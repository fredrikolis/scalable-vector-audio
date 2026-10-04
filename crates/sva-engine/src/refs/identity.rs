// Concern: content-addresses one node, whatever representation it holds | Non-concern: composing a closed form across a ref (mod.rs) | IO: (NodeId) -> Hash

use sva_formula::{Body, ClosedForm, Hash, NodeId, Var, hash_written_with};

use sva_samples::Params;

use crate::error::EngineError;
use crate::index::Round;
use crate::typing::{Step, SumSlot, Typing, Value, When};

use super::cyclic;

/// What one node is, whatever it holds: its definition and the identities of what it reads,
/// never where a reader places it.
pub fn identity(typing: &Typing, node: NodeId) -> Result<Hash, EngineError> {
    let folds = typing.folds();
    for read in typing.unfolded(node, |id| folds.identity(id).is_some()) {
        if read != node {
            let _ = identity_of(typing, read, &mut Vec::new());
        }
    }
    identity_of(typing, node, &mut Vec::new())
}

/// A cycle's refusal depends on where the walk entered it, so only an identity is kept.
fn identity_of(typing: &Typing, node: NodeId, open: &mut Vec<NodeId>) -> Result<Hash, EngineError> {
    if let Some(held) = typing.folds().identity(node) {
        return Ok(held);
    }
    let found = match typing.sum_slots(node) {
        Some(slots) => {
            let mut sink = Sink::new();
            sink.text("terms");
            for slot in slots {
                sink.hash(match slot {
                    SumSlot::Node(id) if *id == node => built(typing, node, open)?,
                    SumSlot::Node(id) => identity_of(typing, *id, open)?,
                    SumSlot::Retired(_) => continue,
                });
            }
            sink.finish()
        }
        None => built(typing, node, open)?,
    };
    typing.folds().keep_identity(node, found);
    Ok(found)
}

/// A form named as the same form written as a node is, each ref by `named` with the variable
/// it holds: a ref to a form of the other variable reads across its transform.
pub(super) fn written(
    form: &ClosedForm,
    named: &mut dyn FnMut(NodeId) -> Result<(Hash, Var), EngineError>,
) -> Result<Hash, EngineError> {
    let mut refused = None;
    let hash = hash_written_with(form, &mut |id| match named(id) {
        Ok((held, var)) if var == form.var => held,
        Ok((held, _)) => {
            let mut sink = Sink::new();
            sink.text("across");
            sink.hash(held);
            sink.finish()
        }
        Err(e) => {
            refused.get_or_insert(e);
            Hash(0, 0)
        }
    });
    refused.map_or(Ok(hash), Err)
}

/// A subterm named as written, which decides its samples, bound and support alike.
pub(crate) fn subterm_identity(typing: &Typing, form: &ClosedForm) -> Result<Hash, EngineError> {
    written(form, &mut |id| Ok((identity(typing, id)?, typing.var(id))))
}

/// A solver, its varying fields named by their nodes: one name, held or read to a switch.
pub(super) fn solver(params: &Params, varying: &[(&str, Hash)]) -> Hash {
    let mut held = params.clone();
    for (key, _) in varying {
        *crate::lower::field(&mut held, key).expect("a varying field") = f64::NAN;
    }
    let mut sink = Sink::new();
    sink.text("solver");
    held.words().into_iter().for_each(|w| sink.word(w));
    for (key, at) in varying {
        sink.text(key);
        sink.hash(*at);
    }
    sink.finish()
}

fn built(typing: &Typing, node: NodeId, open: &mut Vec<NodeId>) -> Result<Hash, EngineError> {
    if open.contains(&node) {
        return Err(cyclic(typing, node));
    }
    open.push(node);
    let found = shape(typing, node, open);
    open.pop();
    found
}

/// Its own value's shape over the identities of what it reads.
fn shape(typing: &Typing, node: NodeId, open: &mut Vec<NodeId>) -> Result<Hash, EngineError> {
    let mut sink = Sink::new();
    match typing.value(node) {
        Value::ClosedForm(form) => {
            return written(form, &mut |id| {
                Ok((identity_of(typing, id, open)?, typing.var(id)))
            });
        }
        Value::Read { .. } if let Some(source) = passes(typing, node) => {
            return identity_of(typing, source, open);
        }
        Value::Cast(cast, source) => {
            sink.text(cast.name());
            sink.hash(identity_of(typing, *source, open)?);
        }
        Value::Read { source, at, .. } => {
            sink.text("read");
            sink.hash(identity_of(typing, *source, open)?);
            when(&mut sink, typing, at)?;
        }
        Value::SelfAt { at, .. } => {
            sink.text("self");
            when(&mut sink, typing, at)?;
        }
        Value::Noise(seed) => {
            sink.text("noise");
            sink.word(*seed);
        }
        Value::Solver { params, varying } => {
            let mut read = Vec::with_capacity(varying.len());
            for (key, arg) in varying {
                read.push((*key, identity_of(typing, *arg, open)?));
            }
            return Ok(solver(params, &read));
        }
        Value::Filter {
            shape,
            x,
            cutoff,
            q,
            gain,
        } => {
            sink.text(crate::vocabulary::shape_name(*shape));
            for operand in [x, cutoff, q, gain] {
                sink.hash(identity_of(typing, *operand, open)?);
            }
        }
        Value::Op { name, args } => {
            sink.text(name);
            let mut held = Vec::with_capacity(args.len());
            for arg in args {
                held.push(identity_of(typing, *arg, open)?);
            }
            if matches!(name.as_str(), "+" | "*") {
                sva_formula::either_order(&mut held);
            }
            held.into_iter().for_each(|h| sink.hash(h));
        }
    }
    Ok(sink.finish())
}

/// The node `node` only passes on, read at its own instant and grid: that value itself.
pub(crate) fn passes(typing: &Typing, node: NodeId) -> Option<NodeId> {
    if typing.sum_slots(node).is_some() {
        return None;
    }
    match typing.value(node) {
        Value::Read {
            source,
            at: When::At(time),
            ..
        } if *time == crate::time::Affine::NOW && typing.grid(*source) == typing.grid(node) => {
            Some(*source)
        }
        Value::ClosedForm(form) => match &form.body {
            Body::Node(read) if typing.var(*read) == form.var => Some(*read),
            _ => None,
        },
        _ => None,
    }
}

/// A moving time is named by its closed form, which holds no ref back to the reader.
pub(super) fn when(sink: &mut Sink, typing: &Typing, at: &When) -> Result<(), EngineError> {
    sink.text("at");
    match at {
        When::At(time) => {
            sink.text("time");
            affine(sink, *time);
        }
        When::Moving(id) => sink.hash(identity(typing, *id)?),
        When::Index(index) => exact(sink, *index),
        When::Step(step) => {
            sink.text("step");
            stepped(sink, typing, step)?;
        }
    }
    Ok(())
}

fn exact(sink: &mut Sink, index: crate::index::Index) {
    round(sink, "index", index.round);
    match index.time {
        Some(time) => affine(sink, time),
        None => sink.text("count"),
    }
    sink.word(index.plus as u64);
}

fn stepped(sink: &mut Sink, typing: &Typing, step: &Step) -> Result<(), EngineError> {
    let each = |sink: &mut Sink, what: &str, parts: &[Step]| {
        sink.text(what);
        sink.word(parts.len() as u64);
        parts.iter().try_for_each(|p| stepped(sink, typing, p))
    };
    match step {
        Step::Index(index) => exact(sink, *index),
        Step::Nearest(time, how) => {
            round(sink, "nearest", *how);
            sink.hash(identity(typing, *time)?);
        }
        Step::Add(parts) => each(sink, "sum", parts)?,
        Step::Mul(parts) => each(sink, "product", parts)?,
        Step::Neg(part) => {
            sink.text("negated");
            stepped(sink, typing, part)?;
        }
    }
    Ok(())
}

fn round(sink: &mut Sink, what: &str, round: Round) {
    let how = match round {
        Round::Even => "",
        Round::Floor => " floor",
        Round::Ceil => " ceil",
    };
    sink.text(&format!("{what}{how}"));
}

fn affine(sink: &mut Sink, time: crate::time::Affine) {
    for q in [time.scale, time.shift] {
        rational(sink, q);
    }
}

/// Both words of each side: a denominator reaches past 64 bits.
fn rational(sink: &mut Sink, q: crate::time::Q) {
    for side in [q.num(), q.den()] {
        sink.word(side as u64);
        sink.word((side >> 64) as u64);
    }
}

const IDENTITY_ROTATE: u32 = 23;

pub(super) struct Sink(sva_formula::Lanes<IDENTITY_ROTATE>);

impl Sink {
    pub(super) fn new() -> Sink {
        Sink(sva_formula::Lanes::default())
    }

    pub(super) fn word(&mut self, part: u64) {
        self.0.word(part);
    }

    pub(super) fn text(&mut self, what: &str) {
        self.word(what.len() as u64);
        for byte in what.as_bytes() {
            self.word(u64::from(*byte));
        }
    }

    pub(super) fn hash(&mut self, held: Hash) {
        self.word(held.0);
        self.word(held.1);
    }

    pub(super) fn finish(&self) -> Hash {
        self.0.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{Sink, rational};
    use crate::time::Q;

    fn named(q: Q) -> sva_formula::Hash {
        let mut sink = Sink::new();
        rational(&mut sink, q);
        sink.finish()
    }

    /// Two denominators alike in their low 64 bits name two rationals.
    #[test]
    fn a_rational_is_named_by_its_whole_denominator() {
        let low = (1_i128 << 64) + 3;
        let a = Q::new(1, 3).expect("a rational");
        let b = Q::new(1, low).expect("a rational");
        assert_ne!(named(a), named(b));
    }
}
