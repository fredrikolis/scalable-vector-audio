// Concern: what a window of the root asks of each value, met with its zeros and carried through each read's map | Non-concern: computing it (eval.rs) | IO: (values, windows) -> a Need per value

use sva_samples::{Extent, NodeRenderer, Slot};

use super::program::leaves;
use super::segments::Segments;
use super::value::{Kind, Program, Value};

/// What readers read of a value, and what it computes to answer them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Need {
    pub(crate) hold: Segments,
    pub(crate) compute: Segments,
    /// A stateful value asked for a past it no longer holds steps again from its start.
    pub(crate) restart: bool,
}

/// Readers first, so a value is asked everything before it asks its own reads.
pub(crate) fn demand(values: &[Value], asked: &[(usize, Extent)]) -> Vec<Need> {
    let mut holds = vec![Segments::default(); values.len()];
    for (v, window) in asked {
        holds[*v].add(*window);
    }
    let mut needs = vec![Need::default(); values.len()];
    for v in (0..values.len()).rev() {
        let value = &values[v];
        let mut hold = holds[v].intersect(value.support);
        if let Some(period) = value.period {
            hold = hold.folded(period);
        }
        let (compute, restart) = match &value.kind {
            Kind::Program(program) if program.stateful() => stateful(value, program, &hold),
            Kind::Frames { .. } | Kind::Istft | Kind::Spectrum(_) => whole(value, &hold),
            _ => (hold.minus(&value.holding()), false),
        };
        match &value.kind {
            Kind::Program(program) => {
                for segment in compute.iter() {
                    for (slot, image) in images(program, segment).into_iter().enumerate() {
                        holds[value.reads[slot]].union(&image);
                    }
                }
            }
            Kind::Frames { .. } | Kind::Istft if !compute.is_empty() => {
                let source = value.reads[0];
                holds[source].add(values[source].support);
            }
            _ => {}
        }
        needs[v] = Need {
            hold,
            compute,
            restart,
        };
    }
    needs
}

fn stateful(value: &Value, program: &Program, hold: &Segments) -> (Segments, bool) {
    if hold.is_empty() {
        return (Segments::default(), false);
    }
    let first = hold.hull().start;
    let start = program.start.expect("a stateful program").min(first);
    let last = hold.hull().end;
    let (from, restart) = match value.end() {
        None => (start, true),
        Some(end) => {
            let before = hold.intersect(Extent::new(i64::MIN, end));
            match value.holding().covers(&before) {
                true => (end, false),
                false => (start, true),
            }
        }
    };
    match from < last {
        true => (Segments::of(Extent::new(from, last)), restart),
        false => (Segments::default(), false),
    }
}

fn whole(value: &Value, hold: &Segments) -> (Segments, bool) {
    let over = match value.support.is_bounded() {
        true => value.support,
        false => hold.hull(),
    };
    match hold.is_empty() || !value.holding().is_empty() {
        true => (Segments::default(), false),
        false => (Segments::of(over), false),
    }
}

/// Only the reads a span keeps are read there.
pub(crate) fn images(program: &Program, over: Extent) -> Vec<Segments> {
    let mut out = vec![Segments::default(); program.layout.read_widths.len()];
    for span in program.spanned.spans() {
        let met = over.intersect(Extent::new(span.from, span.to));
        if met.is_empty() {
            continue;
        }
        leaves(&span.renderer, &mut |leaf| match leaf {
            NodeRenderer::Read {
                slot: Slot::Read(at),
                map,
            } => out[at.0 as usize].add(map.image(met)),
            NodeRenderer::Indexed {
                slot: Slot::Read(at),
                reach,
                ..
            } => {
                let (least, most) = reach.unwrap_or((i64::MIN, i64::MAX));
                let from = met.start.saturating_add(least);
                let to = met.end.saturating_add(most).max(from);
                out[at.0 as usize].add(Extent::new(from, to));
            }
            _ => {}
        });
    }
    out
}
