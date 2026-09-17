// Concern: runs each addend of a sum on its own row and adds the planes | Non-concern: choosing the rows (plan.rs), running one (collapse.rs) | IO: (Vec<Plan>) -> (Buffer, Label)

use super::{AliasScore, Horizon, plan::Plan};
use crate::buffer::Buffer;
use crate::error::CollapseError;
use crate::label::{Detail, Label, Source};
use crate::profile::Profile;

/// Each addend takes the row its own form names; the planes add.
pub fn added(
    parts: Vec<Plan>,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    len: usize,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    let mut collapsed = Vec::with_capacity(parts.len());
    for part in parts {
        collapsed.push(super::run(part, rate, horizon, profile, len, score)?);
    }
    let width = collapsed
        .iter()
        .map(|(buffer, _)| buffer.width)
        .max()
        .expect("a sum row is built from its addends, and a sum has at least one");
    let mut planes = vec![vec![0.0; len]; width];
    for (buffer, _) in &collapsed {
        for (c, plane) in planes.iter_mut().enumerate() {
            // One component broadcasts into every lane of the sum.
            let lane = match buffer.width {
                1 => 0,
                _ => c,
            };
            if lane >= buffer.width {
                continue;
            }
            for (out, held) in plane.iter_mut().zip(buffer.plane(lane)) {
                *out += held;
            }
        }
    }
    let labels: Vec<Label> = collapsed.into_iter().map(|(_, label)| label).collect();
    Ok((
        Buffer {
            rate,
            origin_secs: horizon.start_secs,
            width,
            planes,
        },
        label(&labels, profile, rate),
    ))
}

/// One measured addend measures the whole.
fn label(parts: &[Label], profile: &Profile, rate: u32) -> Label {
    let source = match parts.iter().all(|p| p.source == Source::Exact) {
        true => Source::Exact,
        false => Source::Measured,
    };
    Label::new(
        source,
        profile.name,
        rate,
        Detail::Added {
            parts: parts.iter().map(|p| p.detail.clone()).collect(),
        },
    )
}
