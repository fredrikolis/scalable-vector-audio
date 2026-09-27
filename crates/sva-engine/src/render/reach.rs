// Concern: decides the root's range and every extent under it, each node cut at the decay floor | Non-concern: where a node is cut (cut/), computing a sample | IO: (&mut Render) -> a range

use sva_formula::NodeId;
use sva_samples::Extent;

use super::Render;
use super::cut;
use super::extent::{self, Supports};
use crate::error::{Diagnostic, EngineError, Located};
use crate::query::Representation;

/// A reading of samples, or a count of what they cost, needs the range; a closed form's lines
/// and a node's structure never do.
fn needed(held: &Render) -> bool {
    !held.schedule.materialize.is_empty()
        || held
            .config
            .asks
            .iter()
            .any(|ask| ask.representation == Representation::Flops)
}

/// An envelope needs a range only where its form reaches no symbolic one. Only `audio`, what
/// audio out holds, is cut, so the cuts are the same whatever is read.
pub(super) fn ranged(
    held: &mut Render,
    audio: &[NodeId],
    costed: &[NodeId],
) -> Result<bool, EngineError> {
    if !needed(held) {
        let envelope = held
            .config
            .asks
            .iter()
            .any(|ask| matches!(ask.representation, Representation::Envelope { .. }));
        if !envelope {
            return Ok(false);
        }
        return match decided(held, audio, costed, Ends::Refused) {
            Ok(()) => Ok(true),
            Err(refused) => {
                held.unranged = Some(refused);
                Ok(false)
            }
        };
    }
    decided(held, audio, costed, Ends::Refused).map(|()| true)
}

/// A root never cut streams for as long as it is pulled.
pub(super) fn streamed(held: &mut Render, audio: &[NodeId]) -> Result<(), EngineError> {
    decided(held, audio, audio, Ends::Pulled)
}

enum Ends {
    Refused,
    Pulled,
}

fn decided(
    held: &mut Render,
    audio: &[NodeId],
    costed: &[NodeId],
    ends: Ends,
) -> Result<(), EngineError> {
    let (rate, root) = (held.config.rate, held.root);
    let support = Supports::new(&held.tys, rate).of(root);
    let start = held
        .config
        .range
        .start
        .unwrap_or_else(|| extent::default_start(support));
    let decision = cut::decide(held, audio, start)?;
    let cut_support = Supports::cut(&held.tys, rate, decision.at.clone()).of(root);
    let end = match held.config.range.end.or(extent::default_end(cut_support)) {
        Some(end) => end,
        None => match ends {
            Ends::Pulled => i64::MAX,
            Ends::Refused => return Err(decision.endless.unwrap_or_else(|| endless(held))),
        },
    };
    held.proofs = decision.passes;
    held.proof_ops = decision.ops;
    held.cuts = decision.report;
    held.extents.cuts = decision.at;
    extend(held, costed, Extent::new(start, end.max(start)))
}

pub(super) fn extend(
    held: &mut Render,
    costed: &[NodeId],
    range: Extent,
) -> Result<(), EngineError> {
    let mut demands = vec![(held.root, range)];
    let rows = &held.schedule.rows;
    let measured = match rows.is_empty() {
        true => None,
        false => super::answer::ledger_reads(held),
    };
    demands.extend(
        held.schedule
            .wanted
            .iter()
            .filter(|id| !rows.contains(id) || measured.as_ref().is_none_or(|m| m.contains(id)))
            .map(|id| (*id, range)),
    );
    let cuts = std::mem::take(&mut held.extents.cuts);
    held.extents = extent::decide(held, costed, &demands, &cuts)?;
    held.range = Some(range);
    Ok(())
}

fn endless(held: &Render) -> EngineError {
    let name = held.tys.name(held.root);
    EngineError::refused(Diagnostic {
        code: "render.no_end".to_string(),
        message: format!(
            "`{name}` is read over an interval with no end, and its extent never ends"
        ),
        location: Located::at(name, None),
        help: "give the interval an end, as `[0, 2s]`".to_string(),
    })
}
