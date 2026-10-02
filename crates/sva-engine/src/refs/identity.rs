// Concern: content-addresses one node, whatever representation it holds | Non-concern: composing a closed form across a ref (mod.rs) | IO: (NodeId) -> Hash

use sva_formula::{
    ClosedForm, Hash, NodeId, Var, hash_closed_form, hash_closed_form_with, hash_spectral_sum,
    normalize_closed_form,
};

use crate::error::EngineError;
use crate::index::Round;
use crate::typing::{Step, SumSlot, Typing, Value, When};

use super::{cyclic, nodes_in, spectral_sum_of};

/// What keys a closed form's spectral sum wherever a reading composes one.
pub fn symbolic_hash(typing: &Typing, node: NodeId, want: Var) -> Result<Hash, EngineError> {
    spectral_sum_of(typing, node, want).map(|n| hash_spectral_sum(&n))
}

/// What one node is, whatever it holds: its definition and the identities of what it reads,
/// never where a reader places it.
pub fn identity(typing: &Typing, node: NodeId) -> Result<Hash, EngineError> {
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

/// A closed form that reads no other node: its own spectral sum, where it has one, so two
/// spellings of one form are one value; else its written form.
pub(crate) fn formula_identity(form: &ClosedForm) -> Hash {
    match normalize_closed_form(form) {
        Ok(sum) => hash_spectral_sum(&sum),
        Err(_) => hash_closed_form(form),
    }
}

fn built(typing: &Typing, node: NodeId, open: &mut Vec<NodeId>) -> Result<Hash, EngineError> {
    if open.contains(&node) {
        return Err(cyclic(typing, node));
    }
    open.push(node);
    let mut sink = Sink::new();
    match typing.value(node) {
        Value::ClosedForm(form) if nodes_in(&form.body).is_empty() => {
            open.pop();
            return Ok(formula_identity(form));
        }
        Value::ClosedForm(form) => {
            let mut refused = None;
            let mut read = |id: NodeId| match identity_of(typing, id, open) {
                Ok(held) => held,
                Err(e) => {
                    refused.get_or_insert(e);
                    Hash(0, 0)
                }
            };
            sink.text("closed form");
            sink.hash(hash_closed_form_with(form, &mut read));
            if let Some(e) = refused {
                return Err(e);
            }
        }
        Value::Cast(cast, source) => {
            sink.text(cast.name());
            sink.hash(identity_of(typing, *source, open)?);
        }
        Value::Read { source, at, .. } => {
            sink.text("read");
            sink.hash(identity_of(typing, *source, open)?);
            when(&mut sink, typing, at);
        }
        Value::SelfAt { at, .. } => {
            sink.text("self");
            when(&mut sink, typing, at);
        }
        Value::Noise(seed) => {
            sink.text("noise");
            sink.word(*seed);
        }
        Value::Stored(held) => {
            sink.text("stored");
            sink.hash(held.key);
        }
        Value::Solver { params, varying } => {
            let mut held = (**params).clone();
            for (key, _) in varying {
                *crate::lower::field(&mut held, key).expect("a varying field") = f64::NAN;
            }
            sink.text(&format!("{held:?}"));
            for (key, arg) in varying {
                sink.text(key);
                sink.hash(identity_of(typing, *arg, open)?);
            }
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
            for arg in args {
                sink.hash(identity_of(typing, *arg, open)?);
            }
        }
    }
    open.pop();
    Ok(sink.finish())
}

/// A moving time is named by its closed form, which holds no ref back to the reader.
pub(super) fn when(sink: &mut Sink, typing: &Typing, at: &When) {
    sink.text("at");
    match at {
        When::At(time) => {
            sink.text("time");
            for q in [time.scale, time.shift] {
                sink.word(q.num() as u64);
                sink.word((q.num() >> 64) as u64);
                sink.word(q.den() as u64);
                sink.word((q.den() >> 64) as u64);
            }
        }
        When::Moving(id) => moving(sink, typing, *id),
        When::Index(index) => exact(sink, *index),
        When::Step(step) => {
            sink.text("step");
            stepped(sink, typing, step);
        }
    }
}

fn moving(sink: &mut Sink, typing: &Typing, id: NodeId) {
    match identity(typing, id) {
        Ok(held) => sink.hash(held),
        Err(_) => sink.text(typing.name(id)),
    }
}

fn exact(sink: &mut Sink, index: crate::index::Index) {
    round(sink, "index", index.round);
    match index.time {
        Some(time) => affine(sink, time),
        None => sink.text("count"),
    }
    sink.word(index.plus as u64);
}

fn stepped(sink: &mut Sink, typing: &Typing, step: &Step) {
    let each = |sink: &mut Sink, what: &str, parts: &[Step]| {
        sink.text(what);
        sink.word(parts.len() as u64);
        parts.iter().for_each(|p| stepped(sink, typing, p));
    };
    match step {
        Step::Index(index) => exact(sink, *index),
        Step::Nearest(time, how) => {
            round(sink, "nearest", *how);
            moving(sink, typing, *time);
        }
        Step::Add(parts) => each(sink, "sum", parts),
        Step::Mul(parts) => each(sink, "product", parts),
        Step::Neg(part) => {
            sink.text("negated");
            stepped(sink, typing, part);
        }
    }
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

fn rational(sink: &mut Sink, q: crate::time::Q) {
    sink.word(q.num() as u64);
    sink.word((q.num() >> 64) as u64);
    sink.word(q.den() as u64);
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
