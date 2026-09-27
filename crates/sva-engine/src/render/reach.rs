// Concern: decides the root's range and every extent under it | Non-concern: where a node's support ends (extent.rs), computing a sample | IO: (&mut Render) -> a range

use sva_formula::NodeId;
use sva_samples::Extent;

use super::Render;
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

/// An envelope needs a range only where its form reaches no symbolic one.
pub(super) fn ranged(held: &mut Render, costed: &[NodeId]) -> Result<bool, EngineError> {
    if !needed(held) {
        let envelope = held
            .config
            .asks
            .iter()
            .any(|ask| matches!(ask.representation, Representation::Envelope { .. }));
        if !envelope {
            return Ok(false);
        }
        return match decided(held, costed, Ends::Refused) {
            Ok(()) => Ok(true),
            Err(refused) => {
                held.unranged = Some(refused);
                Ok(false)
            }
        };
    }
    decided(held, costed, Ends::Refused).map(|()| true)
}

/// A root whose support never ends streams for as long as it is pulled.
pub(super) fn streamed(held: &mut Render, audio: &[NodeId]) -> Result<(), EngineError> {
    decided(held, audio, Ends::Pulled)
}

enum Ends {
    Refused,
    Pulled,
}

/// An unstated end is where the root's support ends.
fn decided(held: &mut Render, costed: &[NodeId], ends: Ends) -> Result<(), EngineError> {
    let support = Supports::new(&held.tys, held.config.rate).of(held.root);
    let start = held
        .config
        .range
        .start
        .unwrap_or_else(|| extent::default_start(support));
    let end = match held.config.range.end.or(extent::default_end(support)) {
        Some(end) => end,
        None => match ends {
            Ends::Pulled => i64::MAX,
            Ends::Refused => return Err(endless(held)),
        },
    };
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
    held.extents = extent::decide(held, costed, &demands)?;
    held.range = Some(range);
    Ok(())
}

fn endless(held: &Render) -> EngineError {
    let name = held.tys.name(held.root);
    EngineError::refused(Diagnostic {
        code: "render.no_end".to_string(),
        message: format!(
            "`{name}` is read over an interval with no end, and its support never ends"
        ),
        location: Located::at(name, None),
        help: "give the interval an end, as `[0, 2s]`, or crop it".to_string(),
    })
}
