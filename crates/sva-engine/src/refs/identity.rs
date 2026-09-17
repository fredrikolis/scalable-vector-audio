// Concern: content-addresses one node, whatever representation it holds | Non-concern: composing a closed form across a ref (mod.rs) | IO: (NodeId) -> Hash

use sva_formula::{
    ClosedForm, Hash, NodeId, SpectralSum, Var, hash_closed_form, hash_spectral_sum,
};

use crate::error::{Diagnostic, EngineError};
use crate::offset::Offset;
use crate::typing::{Typing, Value};

use super::{cyclic, nodes_in, spectral_sum_of, substituted_closed_form};

/// What keys a closed form's spectral sum and every buffer collapsed from it.
pub fn symbolic_hash(typing: &Typing, node: NodeId, want: Var) -> Result<Hash, EngineError> {
    spectral_sum_of(typing, node, want).map(|n| hash_spectral_sum(&n))
}

/// What one node is, whatever it holds: its closed form's own form, else the tree it was built from.
pub fn identity(typing: &Typing, node: NodeId) -> Result<Hash, EngineError> {
    identity_of(typing, node, &mut Vec::new())
}

fn identity_of(typing: &Typing, node: NodeId, open: &mut Vec<NodeId>) -> Result<Hash, EngineError> {
    if typing.ty(node).is_closed_form() {
        let named = closed_form_identity(
            &spectral_sum_of(typing, node, typing.var(node)),
            substituted_closed_form(typing, node).as_ref(),
        );
        return match named {
            Ok(hash) => Ok(hash),
            Err(form) => built(typing, node, open).map_err(|tree| neither(form, tree)),
        };
    }
    built(typing, node, open)
}

/// Neither form names the node. Each refusal alone reads as the whole reason, so the one
/// that answers carries both.
fn neither(form: EngineError, tree: EngineError) -> EngineError {
    EngineError::refused(Diagnostic {
        code: tree.code().to_string(),
        message: format!("{form} and the tree it was built from: {tree}"),
        location: tree.at().or_else(|| form.at()).cloned().unwrap_or_default(),
        help: "write the construct inside the node it reads, or sample it".to_string(),
    })
}

fn built(typing: &Typing, node: NodeId, open: &mut Vec<NodeId>) -> Result<Hash, EngineError> {
    if open.contains(&node) {
        return Err(cyclic(typing, node));
    }
    open.push(node);
    let mut sink = Sink::new();
    match typing.value(node) {
        Value::ClosedForm(form) => {
            sink.text("closed form");
            sink.hash(hash_closed_form(form));
            for id in nodes_in(&form.body) {
                sink.hash(identity_of(typing, id, open)?);
            }
        }
        Value::Cast(cast, source) => {
            sink.text(cast.name());
            sink.hash(identity_of(typing, *source, open)?);
        }
        Value::Read { source, at, .. } => {
            sink.text("read");
            sink.hash(identity_of(typing, *source, open)?);
            match at {
                Offset::Steps(steps) => {
                    sink.text("sp");
                    sink.word(*steps as u64);
                }
                Offset::Secs(secs) => {
                    sink.text("s");
                    sink.word(secs.to_bits());
                }
            }
        }
        Value::SelfAt(delay) => sink.text(&format!("self {delay:?}")),
        Value::Grid(count) => sink.text(&format!("sp {count}")),
        Value::Solver(params) => sink.text(&format!("{params:?}")),
        Value::Filter {
            shape,
            x,
            cutoff,
            q,
            gain,
        } => {
            sink.text(shape.name());
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

const IDENTITY_ROTATE: u32 = 23;

struct Sink(sva_formula::Lanes<IDENTITY_ROTATE>);

impl Sink {
    fn new() -> Sink {
        Sink(sva_formula::Lanes::default())
    }

    fn word(&mut self, part: u64) {
        self.0.word(part);
    }

    fn text(&mut self, what: &str) {
        self.word(what.len() as u64);
        for byte in what.as_bytes() {
            self.word(u64::from(*byte));
        }
    }

    fn hash(&mut self, held: Hash) {
        self.word(held.0);
        self.word(held.1);
    }

    fn finish(&self) -> Hash {
        self.0.finish()
    }
}

/// The same identity from forms a caller already holds, so a render never walks a closed form twice.
pub fn closed_form_identity(
    sum: &Result<SpectralSum, EngineError>,
    written: Option<&ClosedForm>,
) -> Result<Hash, EngineError> {
    match (sum, written) {
        (Ok(sum), _) => Ok(hash_spectral_sum(sum)),
        (Err(_), Some(form)) => Ok(hash_closed_form(form)),
        (Err(e), None) => Err(e.clone()),
    }
}
