// Concern: decides the root's range, the proofs that end it and the extents under it | Non-concern: the condition's own arithmetic (until.rs), computing a sample | IO: (&mut Render) -> a range, a stop

use sva_formula::{Hash, NodeId};
use sva_samples::{Buffer, Extent};

use super::extent::{self, Supports};
use super::silent::{self, Proven};
use super::until::Known;
use super::{Render, Until};
use crate::cache::{Expected, Lens, Payload, Recording};
use crate::error::{Diagnostic, EngineError, Located};
use crate::query::Representation;

/// What the render was decided over before any sample, and what already answers its stop.
pub(super) struct Reached {
    proven: Vec<Proven>,
    stop: Option<i64>,
    key: Option<Hash>,
    /// Where the root's support ends, from where on every sample is exactly zero.
    silent_from: Option<i64>,
}

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

/// A closed form's envelope reads samples only where its own form reaches no symbolic one, so
/// a range for it is decided where one can be, and is no refusal where none can.
pub(super) fn ranged(
    held: &mut Render,
    recording: Option<&Recording>,
    identity: Option<(Hash, bool)>,
    volatile_root: bool,
    costed: &[NodeId],
) -> Result<Option<Reached>, EngineError> {
    if !needed(held) {
        let envelope = held
            .config
            .asks
            .iter()
            .any(|ask| matches!(ask.representation, Representation::Envelope { .. }));
        if !envelope {
            return Ok(None);
        }
        return match decided(held, recording, identity, volatile_root, costed) {
            Ok(reached) => Ok(Some(reached)),
            Err(refused) => {
                held.unranged = Some(refused);
                Ok(None)
            }
        };
    }
    decided(held, recording, identity, volatile_root, costed).map(Some)
}

fn decided(
    held: &mut Render,
    recording: Option<&Recording>,
    identity: Option<(Hash, bool)>,
    volatile_root: bool,
    costed: &[NodeId],
) -> Result<Reached, EngineError> {
    let support = Supports::new(&held.tys, held.config.rate).of(held.root);
    let start = held
        .config
        .range
        .start
        .unwrap_or_else(|| extent::default_start(support));
    let key = identity
        .filter(|_| !volatile_root && held.config.until.is_some())
        .map(|identity| stop_key(identity, held, start));
    let lens = recording.map(|r| r.at(None, false, true));
    let (end, proven, stop) = match recalled(lens.as_ref(), key, held) {
        Some((end, stop)) => (end, Vec::new(), Some(stop)),
        None => {
            let (end, proven) = ended(held, start, support)?;
            (end, proven, None)
        }
    };
    let range = Extent::new(start, end.max(start));
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
    Ok(Reached {
        proven,
        stop,
        key,
        silent_from: extent::default_end(support),
    })
}

/// The root cut to the first sample `until` holds at, which is where the render ends.
pub(super) fn stopped(held: &mut Render, reached: Reached, recording: Option<&Recording>) {
    let range = held.range.expect("a reached render holds its range");
    let root = held
        .output(held.root)
        .or_else(|| super::answer::on_the_grid(held, held.root).ok());
    let stop = reached
        .stop
        .unwrap_or_else(|| match (&held.config.until, root) {
            (Some(until), Some(root)) => {
                let beyond = match reached.silent_from {
                    Some(silent) if range.end >= silent => 0.0,
                    _ => reached
                        .proven
                        .iter()
                        .map(|p| p.from(range.end))
                        .fold(f64::INFINITY, f64::min),
                };
                let known = Known::new(
                    root.plane(0),
                    range.start,
                    range.start,
                    held.config.rate,
                    beyond,
                );
                until
                    .first(&known, range.start, range.end)
                    .unwrap_or(range.end)
            }
            _ => range.end,
        });
    held.range = Some(Extent::new(range.start, stop));
    if let (Some(key), None, Some(recording)) = (reached.key, reached.stop, recording) {
        let record = Buffer::mono(held.config.rate, vec![range.end as f64, stop as f64]);
        recording
            .at(None, false, true)
            .store(key, &Payload::Samples(Box::new(record)), None);
    }
}

/// The range's end, where the root's support ends where the range states none, or the first
/// sample `until` is proven to hold at, whichever is first.
fn ended(
    held: &mut Render,
    start: i64,
    support: Extent,
) -> Result<(i64, Vec<Proven>), EngineError> {
    let config = &held.config;
    let rate = f64::from(config.rate);
    let last = config.range.end.or(extent::default_end(support));
    let Some(until) = &config.until else {
        return match last {
            Some(end) => Ok((end, Vec::new())),
            None => Err(no_stop(held, None, None)),
        };
    };
    let limit = last.unwrap_or(start + (config.proof_limit_secs * rate).ceil() as i64);
    let mut levels = Vec::new();
    until.levels(&mut levels);
    levels.sort_by(f64::total_cmp);
    levels.dedup();
    // A proof bounds the state a render starts with only from where that state starts.
    let first = start.min(extent::default_start(support));
    let (mut proven, mut failed, mut proofs) = (Vec::new(), None, 0);
    for level in levels {
        let (found, ran) = silent::proven_at(&held.tys, held.root, config, first, level, limit);
        proofs += ran;
        match found {
            Ok(at) => proven.push((level, at)),
            Err(e) => {
                failed.get_or_insert(e);
            }
        }
    }
    let quiet = |level: f64| {
        proven
            .iter()
            .find(|(held, _)| *held == level)
            .map(|(_, p)| p.at)
    };
    let provable = until.provable(start, config.rate, &quiet).map(|(at, _)| at);
    let end = match (last, provable) {
        (Some(end), Some(at)) => end.min(at),
        (Some(end), None) => end,
        (None, Some(at)) => at,
        (None, None) => return Err(no_stop(held, Some(until), failed)),
    };
    held.proofs = proofs;
    Ok((end, proven.into_iter().map(|(_, p)| p).collect()))
}

/// An open range that nothing proves an end for, in the words of the proof that failed.
fn no_stop(held: &Render, until: Option<&Until>, failed: Option<EngineError>) -> EngineError {
    let name = held.tys.name(held.root);
    let Some(until) = until else {
        return EngineError::refused(Diagnostic {
            code: "render.no_stop".to_string(),
            message: format!(
                "`{name}` is read over an interval with no end, and its support never ends"
            ),
            location: Located::at(name, None),
            help: "give the interval an end, as `[0, 2s]`, or --until a condition that ends \
                   it, as `max(envelope([t, inf))) < -96db`"
                .to_string(),
        });
    };
    let condition = format!("`{until}`");
    match failed {
        Some(EngineError::Refused(mut d)) => {
            d.message = format!("{condition} is never proven to hold: {}", d.message);
            EngineError::Refused(d)
        }
        Some(other) => other,
        None => EngineError::refused(Diagnostic {
            code: "render.no_stop".to_string(),
            message: format!(
                "`{name}` is read over an interval with no end, and {condition} is never \
                 proven to hold"
            ),
            location: Located::at(name, None),
            help: "give the interval an end, as `[0, 2s]`, or --until a condition a proof \
                   brings about"
                .to_string(),
        }),
    }
}

const STOP_TAG: u64 = 0x73_74_6f_70_00_00_00_01;

/// The root at one rate and start, ended by one condition: the end and stop are its own.
fn stop_key((identity, scored): (Hash, bool), held: &Render, start: i64) -> Hash {
    let config = &held.config;
    let until = config
        .until
        .as_ref()
        .map_or(String::new(), Until::to_string);
    let mut words = vec![
        u64::from(config.rate),
        start as u64,
        config.range.end.map_or(u64::MAX, |end| end as u64),
        config.proof_limit_secs.to_bits(),
        u64::from(scored),
        STOP_TAG,
    ];
    words.extend(until.bytes().map(u64::from));
    crate::cache::mixed(identity, &words)
}

fn recalled(cache: Option<&Lens>, key: Option<Hash>, held: &Render) -> Option<(i64, i64)> {
    let expected = Expected::Samples {
        rate: held.config.rate,
        width: 1,
        samples: 2,
    };
    let entry = cache?.load(key?, held.tys.name(held.root), expected)?;
    let record = entry.payload.samples()?.plane(0).to_vec();
    Some((record[0] as i64, record[1] as i64))
}
