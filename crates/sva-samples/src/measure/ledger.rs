// Concern: attributes a target's energy down its ref tree, one entry per node | Non-concern: rendering the buffers it reads | IO: (buffers, deps, kinds, target) -> entries

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::buffer::Buffer;
use crate::measure::envelope::rms;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Amplitude,
    Hertz,
    Q,
    Decibels,
    Seconds,
    Radians,
    Ratio,
}

impl Unit {
    pub fn name(self) -> &'static str {
        match self {
            Unit::Amplitude => "amplitude",
            Unit::Hertz => "hz",
            Unit::Q => "q",
            Unit::Decibels => "db",
            Unit::Seconds => "seconds",
            Unit::Radians => "radians",
            Unit::Ratio => "ratio",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalKind {
    Audio,
    Control(Unit),
}

impl SignalKind {
    pub fn is_audio(self) -> bool {
        self == SignalKind::Audio
    }

    pub fn unit(self) -> Unit {
        match self {
            SignalKind::Audio => Unit::Amplitude,
            SignalKind::Control(u) => u,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LedgerEntry {
    pub node: String,
    pub channel: Option<usize>,
    pub depth: usize,
    pub kind: SignalKind,
    pub rms: f64,
    pub peak: f64,
    pub min: f64,
    pub max: f64,
    /// The part of its reader's own energy this node accounts for, so one reader's refs add
    /// to one over a sum. `None` for a non-audio kind, and for a ref no addend isolates.
    pub share: Option<f64>,
    /// A sample outside [-1, 1], the full scale a `.wav` destination clamps to, so a peak of
    /// exactly 1.0 is not clipped.
    pub clipped: Option<bool>,
}

/// One entry per component, off what each node gave the node reading it.
pub fn attribute(
    buffers: &BTreeMap<String, Buffer>,
    contributed: &BTreeMap<String, Buffer>,
    deps: &BTreeMap<String, Vec<String>>,
    kinds: &BTreeMap<String, SignalKind>,
    target: &str,
    range: std::ops::Range<usize>,
    depth_limit: usize,
) -> Vec<LedgerEntry> {
    let held = |path: &str| contributed.get(path).or_else(|| buffers.get(path));
    let slice = |path: &str, c: usize| -> std::borrow::Cow<'_, [f64]> {
        held(path).map_or(std::borrow::Cow::Borrowed(&[][..]), |b| {
            b.window(c, range.clone())
        })
    };
    let width = |path: &str| held(path).map_or(1, |b| b.width);
    // Over a sum the parts are the whole: one reader's refs add to exactly one.
    let share_of = |reader: &str, x: &[f64], c: usize| -> f64 {
        let Some(buffer) = buffers.get(reader).filter(|b| b.width > 0) else {
            return 0.0;
        };
        let y = buffer.window(c % buffer.width, range.clone());
        let energy: f64 = y.iter().map(|&v| v * v).sum();
        match energy > 0.0 {
            true => x.iter().zip(y.iter()).map(|(a, b)| a * b).sum::<f64>() / energy,
            false => 0.0,
        }
    };

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<(&str, usize, &str)> = VecDeque::new();
    let mut out = Vec::new();
    queue.push_back((target, 0, target));
    seen.insert(target);

    while let Some((path, depth, reader)) = queue.pop_front() {
        // `kinds` names what holds a buffer; a node composed into another holds none.
        let kind = kinds.get(path).copied().unwrap_or(SignalKind::Audio);
        let components = width(path);
        let gated = kind.is_audio();
        let attributed = path == target || contributed.contains_key(path);
        for c in 0..components {
            let buf = slice(path, c);
            let node_rms = rms(&buf);
            let peak = buf.iter().fold(0f64, |a, &x| a.max(x.abs()));
            let min = buf.iter().fold(f64::INFINITY, |a, &x| a.min(x));
            let max = buf.iter().fold(f64::NEG_INFINITY, |a, &x| a.max(x));
            out.push(LedgerEntry {
                node: path.to_string(),
                channel: (components > 1).then_some(c),
                depth,
                kind,
                rms: node_rms,
                peak,
                min,
                max,
                share: (gated && attributed).then(|| share_of(reader, &buf, c)),
                clipped: gated.then_some(peak > 1.0),
            });
        }

        if depth >= depth_limit {
            continue;
        }
        for child in deps.get(path).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.insert(child.as_str()) {
                let key = deps
                    .get_key_value(child.as_str())
                    .map(|(k, _)| k.as_str())
                    .unwrap_or(child.as_str());
                queue.push_back((key, depth + 1, path));
            }
        }
    }
    out
}
