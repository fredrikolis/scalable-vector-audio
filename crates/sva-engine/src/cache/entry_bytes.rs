// Concern: the bytes one persisted entry is, its samples spelled by a codec | Non-concern: where the bytes live (disk.rs, pack.rs), the label encoding (label.rs) | IO: (Buffer, traces, label) <-> bytes

use sva_formula::filter::Shape;
use sva_samples::{AutomationFrame, Buffer, FilterTrace, Label};

use super::{Entry, Payload, Tier};

/// A codec id after the magic makes an older file a miss rather than a misread.
const MAGIC: &[u8; 4] = b"RBC6";

/// A persisted entry's samples. The id is written into every entry, and a store reading an
/// entry another codec wrote answers a miss: the bytes are not wrong, only not its own.
///
/// Only an exact codec, one whose round trip is bit-identical, may implement this until keyed,
/// opt-in approximate storage exists: a lossy one would change the value a key names.
pub trait SampleCodec: Sync {
    fn id(&self) -> u8;

    fn encode(&self, buffer: &Buffer, out: &mut Vec<u8>);

    /// `None` unless `bytes` is exactly `width` planes of `samples` each.
    fn decode(&self, bytes: &[u8], rate: u32, width: usize, samples: usize) -> Option<Buffer>;
}

pub struct RawF64;

impl SampleCodec for RawF64 {
    fn id(&self) -> u8 {
        0
    }

    fn encode(&self, buffer: &Buffer, out: &mut Vec<u8>) {
        out.reserve(buffer.len() * buffer.width * size_of::<f64>());
        for c in 0..buffer.width {
            for &s in buffer.plane(c) {
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
    }

    fn decode(&self, bytes: &[u8], rate: u32, width: usize, samples: usize) -> Option<Buffer> {
        let plane = samples.checked_mul(size_of::<f64>())?;
        if bytes.len() != plane.checked_mul(width)? {
            return None;
        }
        let planes = (0..width)
            .map(|c| {
                bytes[c * plane..(c + 1) * plane]
                    .chunks_exact(8)
                    .map(|b| f64::from_le_bytes(b.try_into().expect("chunks_exact(8)")))
                    .collect()
            })
            .collect();
        Some(Buffer::of_planes(rate, planes))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Unread {
    OtherCodec,
    Damaged,
}

pub(super) struct Writer(pub Vec<u8>);

impl Writer {
    pub(super) fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub(super) fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
}

pub(super) struct Reader<'a>(pub &'a [u8]);

impl<'a> Reader<'a> {
    pub(super) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }
    pub(super) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    pub(super) fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

pub(super) fn encode(
    buffer: &Buffer,
    traces: &[FilterTrace],
    label: Option<&Label>,
    codec: &dyn SampleCodec,
) -> Vec<u8> {
    let mut w = Writer(Vec::with_capacity(buffer.len() * buffer.width * 8 + 64));
    w.0.extend_from_slice(MAGIC);
    w.0.push(codec.id());
    w.u32(buffer.rate);
    w.u64(buffer.len() as u64);
    w.u32(buffer.width as u32);
    write_traces(&mut w, traces);
    super::label::write(&mut w, label);
    codec.encode(buffer, &mut w.0);
    w.0
}

fn write_traces(w: &mut Writer, traces: &[FilterTrace]) {
    w.u32(traces.len() as u32);
    for trace in traces {
        w.u32(trace.site as u32);
        w.u32(trace.channel.map_or(u32::MAX, |c| c as u32));
        w.0.push(u8::from(trace.clamped));
        w.u32(trace.shape.len() as u32);
        w.0.extend_from_slice(trace.shape.as_bytes());
        w.f64(trace.trace_secs);
        w.u32(trace.frames.len() as u32);
        for f in &trace.frames {
            w.f64(f.t_secs);
            w.f64(f.cutoff);
            w.f64(f.q);
            w.f64(f.gain_db);
        }
    }
}

/// Every field is checked against what the caller asked for, not merely parsed: the hash
/// already rules out a mismatch, so one here means the hash's domain is wrong and the only
/// safe answer is to re-render.
pub(super) fn decode(
    bytes: &[u8],
    node: &str,
    (rate, width, samples): (u32, usize, usize),
    codec: &dyn SampleCodec,
) -> Result<Entry, Unread> {
    let mut r = Reader(bytes);
    if r.take(4) != Some(MAGIC) {
        return Err(Unread::Damaged);
    }
    match r.take(1) {
        Some([id]) if *id == codec.id() => {}
        Some(_) => return Err(Unread::OtherCodec),
        None => return Err(Unread::Damaged),
    }
    decode_samples(&mut r, node, rate, width, samples, codec).ok_or(Unread::Damaged)
}

fn decode_samples(
    r: &mut Reader,
    node: &str,
    rate: u32,
    width: usize,
    samples: usize,
    codec: &dyn SampleCodec,
) -> Option<Entry> {
    if r.u32()? != rate || r.u64()? != samples as u64 || r.u32()? != width as u32 {
        return None;
    }
    let traces = read_traces(r, node)?;
    let label = super::label::read(r)?;
    let buffer = codec.decode(r.0, rate, width, samples)?;
    Some(Entry {
        payload: Payload::Samples(Box::new(buffer)),
        traces,
        label,
        tier: Tier::Persistent,
    })
}

fn read_traces(r: &mut Reader, node: &str) -> Option<Vec<FilterTrace>> {
    let mut traces = Vec::new();
    for _ in 0..r.u32()? {
        let site = r.u32()? as usize;
        let channel = match r.u32()? {
            u32::MAX => None,
            c => Some(c as usize),
        };
        let clamped = r.take(1)?[0] == 1;
        let name_len = r.u32()? as usize;
        let shape = Shape::from_name(std::str::from_utf8(r.take(name_len)?).ok()?)?.name();
        let trace_secs = r.f64()?;
        let frame_count = r.u32()? as usize;
        let mut frames = Vec::with_capacity(frame_count.min(1 << 20));
        for _ in 0..frame_count {
            frames.push(AutomationFrame {
                t_secs: r.f64()?,
                cutoff: r.f64()?,
                q: r.f64()?,
                gain_db: r.f64()?,
            });
        }
        traces.push(FilterTrace {
            node: node.to_string(),
            site,
            channel,
            shape,
            clamped,
            trace_secs,
            frames,
        });
    }
    Some(traces)
}
