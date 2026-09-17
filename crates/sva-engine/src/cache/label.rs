// Concern: writes one collapse label into a store's bytes and reads it back | Non-concern: the buffer's own encoding (disk.rs), what a label states (sva-samples) | IO: (&Label) <-> bytes

use sva_samples::{Detail, Dropped, Label, Rule, Source};

use super::disk::{Reader, Writer};

/// A label nested past this is a corrupt file, not a sum anything wrote.
const MAX_DEPTH: usize = 32;

pub fn write(w: &mut Writer, label: Option<&Label>) {
    let Some(label) = label else {
        w.0.push(0);
        return;
    };
    w.0.push(1);
    w.0.push(u8::from(label.source == Source::Measured));
    string(w, label.profile);
    w.u32(label.rate);
    detail(w, &label.detail);
}

pub fn read(r: &mut Reader) -> Option<Option<Label>> {
    if r.take(1)?[0] == 0 {
        return Some(None);
    }
    let source = match r.take(1)?[0] {
        0 => Source::Exact,
        _ => Source::Measured,
    };
    let profile = sva_samples::profile::named(text(r)?)?;
    let rate = r.u32()?;
    Some(Some(Label::new(
        source,
        profile.name,
        rate,
        read_detail(r, 0)?,
    )))
}

fn string(w: &mut Writer, text: &str) {
    w.u32(text.len() as u32);
    w.0.extend_from_slice(text.as_bytes());
}

fn text<'a>(r: &mut Reader<'a>) -> Option<&'a str> {
    let len = r.u32()? as usize;
    std::str::from_utf8(r.take(len)?).ok()
}

fn opt_f64(w: &mut Writer, v: Option<f64>) {
    w.0.push(u8::from(v.is_some()));
    w.f64(v.unwrap_or(0.0));
}

fn read_opt_f64(r: &mut Reader) -> Option<Option<f64>> {
    let held = r.take(1)?[0] == 1;
    let v = r.f64()?;
    Some(held.then_some(v))
}

fn detail(w: &mut Writer, d: &Detail) {
    w.u32(tag(d));
    string(w, d.rule().as_str());
    match d {
        Detail::Lines {
            placed,
            summed,
            dropped,
            dropped_more,
            terms,
            tail_db,
            ..
        } => {
            w.u32(*placed as u32);
            w.u32(*summed as u32);
            w.u32(dropped.len() as u32);
            for line in dropped {
                w.f64(line.hz);
                w.f64(line.db);
            }
            w.u32(*dropped_more as u32);
            w.u32(terms.map_or(u32::MAX, |t| t as u32));
            opt_f64(w, *tail_db);
        }
        Detail::Cropped { tail_db, .. } => opt_f64(w, *tail_db),
        Detail::Point { alias_db, .. } => opt_f64(w, *alias_db),
        Detail::Spectrum { wrap_db, .. } => w.f64(*wrap_db),
        Detail::Roundtrip { edited, .. } => w.0.push(u8::from(*edited)),
        Detail::Added { parts } => {
            w.u32(parts.len() as u32);
            for part in parts {
                detail(w, part);
            }
        }
        Detail::Continuous { .. } | Detail::Reading { .. } => {}
    }
}

fn tag(d: &Detail) -> u32 {
    match d {
        Detail::Lines { .. } => 0,
        Detail::Continuous { .. } => 1,
        Detail::Cropped { .. } => 2,
        Detail::Point { .. } => 3,
        Detail::Spectrum { .. } => 4,
        Detail::Roundtrip { .. } => 5,
        Detail::Reading { .. } => 6,
        Detail::Added { .. } => 7,
    }
}

fn read_detail(r: &mut Reader, depth: usize) -> Option<Detail> {
    if depth > MAX_DEPTH {
        return None;
    }
    let tag = r.u32()?;
    let rule = Rule::named(text(r)?)?;
    Some(match tag {
        0 => Detail::Lines {
            rule,
            placed: r.u32()? as usize,
            summed: r.u32()? as usize,
            dropped: read_dropped(r)?,
            dropped_more: r.u32()? as usize,
            terms: match r.u32()? {
                u32::MAX => None,
                n => Some(n as usize),
            },
            tail_db: read_opt_f64(r)?,
        },
        1 => Detail::Continuous { rule },
        2 => Detail::Cropped {
            rule,
            tail_db: read_opt_f64(r)?,
        },
        3 => Detail::Point {
            rule,
            alias_db: read_opt_f64(r)?,
        },
        4 => Detail::Spectrum {
            rule,
            wrap_db: r.f64()?,
        },
        5 => Detail::Roundtrip {
            rule,
            edited: r.take(1)?[0] == 1,
        },
        6 => Detail::Reading { rule },
        7 => {
            let count = r.u32()? as usize;
            let mut parts = Vec::with_capacity(count.min(1 << 10));
            for _ in 0..count {
                parts.push(read_detail(r, depth + 1)?);
            }
            Detail::Added { parts }
        }
        _ => return None,
    })
}

fn read_dropped(r: &mut Reader) -> Option<Vec<Dropped>> {
    let count = r.u32()? as usize;
    let mut out = Vec::with_capacity(count.min(1 << 10));
    for _ in 0..count {
        out.push(Dropped {
            hz: r.f64()?,
            db: r.f64()?,
        });
    }
    Some(out)
}
