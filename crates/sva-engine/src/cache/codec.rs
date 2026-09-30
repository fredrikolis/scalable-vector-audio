// Concern: writes one stored node value and one staged run of its samples as bytes, and reads them back | Non-concern: where the bytes live or when | IO: (Stored or Buffer) <-> bytes, a corrupt one None

use sva_formula::Codomain;
use sva_samples::{
    Buffer, Cost, Detail, Dropped, Extent, Grid, Label, PSYCHOACOUSTIC_V1, Rule, Source,
};

use super::Stored;

const ENTRY: &[u8; 4] = b"SVAn";
const CHUNK: &[u8; 4] = b"SVAc";

pub(crate) fn entry(stored: &Stored) -> Vec<u8> {
    let mut out = ENTRY.to_vec();
    word(&mut out, stored.key.0);
    word(&mut out, stored.key.1);
    labelled(&mut out, &stored.label);
    out.push(stored.width);
    out.push(match stored.codomain {
        Codomain::Real => 0,
        Codomain::Complex => 1,
    });
    maybe(&mut out, stored.rate.map(u64::from));
    word(&mut out, u64::from(stored.grid.rate));
    out.extend_from_slice(&stored.grid.a.to_le_bytes());
    out.extend_from_slice(&stored.grid.d.to_le_bytes());
    word(&mut out, stored.support.start as u64);
    word(&mut out, stored.support.end as u64);
    out.extend_from_slice(&stored.priced.to_le_bytes());
    word(&mut out, stored.moved.to_bits());
    out.push(u8::from(stored.readable));
    segments(&mut out, &stored.segments);
    sealed(out)
}

/// A truncated, corrupt or foreign entry is `None`, never a partial value.
pub(crate) fn read_entry(bytes: &[u8]) -> Option<Stored> {
    let mut r = Reader(opened(bytes, ENTRY)?);
    let key = sva_formula::Hash(r.word()?, r.word()?);
    let label = r.label()?;
    let width = r.byte()?;
    let codomain = match r.byte()? {
        0 => Codomain::Real,
        1 => Codomain::Complex,
        _ => return None,
    };
    let rate = match r.maybe()? {
        Some(rate) => Some(u32::try_from(rate).ok()?),
        None => None,
    };
    let grid = Grid {
        rate: u32::try_from(r.word()?).ok()?,
        a: r.wide()? as i128,
        d: r.wide()? as i128,
    };
    let (start, end) = (r.word()? as i64, r.word()? as i64);
    let support = (start <= end).then(|| Extent::new(start, end))?;
    let priced = r.wide()?;
    let moved = f64::from_bits(r.word()?);
    let readable = match r.byte()? {
        0 => false,
        1 => true,
        _ => return None,
    };
    let segments = r.segments()?;
    r.0.is_empty().then_some(Stored {
        key,
        segments,
        label,
        width,
        codomain,
        rate,
        grid,
        support,
        priced,
        moved,
        readable,
    })
}

pub(crate) fn chunk(samples: &Buffer) -> Vec<u8> {
    let mut out = CHUNK.to_vec();
    segments(&mut out, std::slice::from_ref(samples));
    sealed(out)
}

pub(crate) fn read_chunk(bytes: &[u8]) -> Option<Buffer> {
    let mut r = Reader(opened(bytes, CHUNK)?);
    let mut parts = r.segments()?;
    (r.0.is_empty() && parts.len() == 1).then(|| parts.remove(0))
}

fn sealed(mut out: Vec<u8>) -> Vec<u8> {
    let sum = checksum(&out);
    word(&mut out, sum);
    out
}

/// The body past `magic`, where the checksum holds.
fn opened<'b>(bytes: &'b [u8], magic: &[u8; 4]) -> Option<&'b [u8]> {
    let body = bytes.len().checked_sub(8)?;
    let (body, sum) = bytes.split_at(body);
    (checksum(body).to_le_bytes() == sum && body.starts_with(magic)).then(|| &body[magic.len()..])
}

fn segments(out: &mut Vec<u8>, parts: &[Buffer]) {
    word(out, parts.len() as u64);
    for part in parts {
        word(out, u64::from(part.rate));
        word(out, part.start as u64);
        word(out, part.width as u64);
        word(out, part.len() as u64);
        for plane in &part.planes {
            for sample in plane {
                word(out, sample.to_bits());
            }
        }
    }
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn word(out: &mut Vec<u8>, w: u64) {
    out.extend_from_slice(&w.to_le_bytes());
}

fn float(out: &mut Vec<u8>, v: Option<f64>) {
    maybe(out, v.map(f64::to_bits));
}

fn maybe(out: &mut Vec<u8>, w: Option<u64>) {
    match w {
        None => out.push(0),
        Some(w) => {
            out.push(1);
            word(out, w);
        }
    }
}

fn text(out: &mut Vec<u8>, s: &str) {
    word(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn labelled(out: &mut Vec<u8>, label: &Label) {
    out.push(match label.source {
        Source::Exact => 0,
        Source::Measured => 1,
    });
    text(out, label.profile);
    word(out, u64::from(label.rate));
    detailed(out, &label.detail);
    match label.cost {
        None => out.push(0),
        Some(Cost { flops, budget }) => {
            out.push(1);
            out.extend_from_slice(&flops.to_le_bytes());
            out.extend_from_slice(&budget.to_le_bytes());
        }
    }
    float(out, label.moved);
}

fn detailed(out: &mut Vec<u8>, detail: &Detail) {
    let rule = |out: &mut Vec<u8>, rule: &Rule| text(out, rule.as_str());
    match detail {
        Detail::Lines {
            rule: r,
            placed,
            summed,
            dropped,
            dropped_more,
            terms,
            tail_db,
        } => {
            out.push(0);
            rule(out, r);
            word(out, *placed as u64);
            word(out, *summed as u64);
            word(out, dropped.len() as u64);
            for d in dropped {
                word(out, d.hz.to_bits());
                word(out, d.db.to_bits());
            }
            word(out, *dropped_more as u64);
            maybe(out, terms.map(|t| t as u64));
            float(out, *tail_db);
        }
        Detail::Continuous { rule: r } => {
            out.push(1);
            rule(out, r);
        }
        Detail::Cropped { rule: r, tail_db } => {
            out.push(2);
            rule(out, r);
            float(out, *tail_db);
        }
        Detail::Point { rule: r, alias_db } => {
            out.push(3);
            rule(out, r);
            float(out, *alias_db);
        }
        Detail::Spectrum { rule: r, wrap_db } => {
            out.push(4);
            rule(out, r);
            word(out, wrap_db.to_bits());
        }
        Detail::Roundtrip { rule: r, edited } => {
            out.push(5);
            rule(out, r);
            out.push(u8::from(*edited));
        }
        Detail::Reading { rule: r } => {
            out.push(6);
            rule(out, r);
        }
        Detail::Added { parts } => {
            out.push(7);
            word(out, parts.len() as u64);
            for part in parts {
                detailed(out, part);
            }
        }
    }
}

struct Reader<'b>(&'b [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn word(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn segments(&mut self) -> Option<Vec<Buffer>> {
        let count = self.word()? as usize;
        let mut parts = Vec::new();
        for _ in 0..count.min(self.0.len()) {
            let rate = u32::try_from(self.word()?).ok()?;
            let start = self.word()? as i64;
            let (width, len) = (self.word()? as usize, self.word()? as usize);
            if width.saturating_mul(len).saturating_mul(8) > self.0.len() {
                return None;
            }
            let planes = (0..width)
                .map(|_| (0..len).map(|_| self.word().map(f64::from_bits)).collect())
                .collect::<Option<Vec<Vec<f64>>>>()?;
            let mut part = Buffer::of_planes(rate, planes);
            part.start = start;
            parts.push(part);
        }
        (parts.len() == count).then_some(parts)
    }

    fn wide(&mut self) -> Option<u128> {
        Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }

    fn maybe(&mut self) -> Option<Option<u64>> {
        match self.byte()? {
            0 => Some(None),
            1 => Some(Some(self.word()?)),
            _ => None,
        }
    }

    fn float(&mut self) -> Option<Option<f64>> {
        Some(self.maybe()?.map(f64::from_bits))
    }

    fn text(&mut self) -> Option<&str> {
        let len = self.word()? as usize;
        std::str::from_utf8(self.take(len)?).ok()
    }

    fn rule(&mut self) -> Option<Rule> {
        Rule::named(self.text()?)
    }

    fn label(&mut self) -> Option<Label> {
        let source = match self.byte()? {
            0 => Source::Exact,
            1 => Source::Measured,
            _ => return None,
        };
        let profile = match self.text()? {
            name if name == PSYCHOACOUSTIC_V1.name => PSYCHOACOUSTIC_V1.name,
            _ => return None,
        };
        let rate = u32::try_from(self.word()?).ok()?;
        let detail = self.detail()?;
        let cost = match self.byte()? {
            0 => None,
            1 => Some(Cost {
                flops: self.wide()?,
                budget: self.wide()?,
            }),
            _ => return None,
        };
        let moved = self.float()?;
        Some(Label {
            source,
            profile,
            rate,
            detail,
            cost,
            moved,
        })
    }

    fn detail(&mut self) -> Option<Detail> {
        Some(match self.byte()? {
            0 => {
                let rule = self.rule()?;
                let (placed, summed) = (self.word()? as usize, self.word()? as usize);
                let count = self.word()? as usize;
                let dropped = (0..count.min(self.0.len()))
                    .map(|_| {
                        Some(Dropped {
                            hz: f64::from_bits(self.word()?),
                            db: f64::from_bits(self.word()?),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                if dropped.len() != count {
                    return None;
                }
                Detail::Lines {
                    rule,
                    placed,
                    summed,
                    dropped,
                    dropped_more: self.word()? as usize,
                    terms: self.maybe()?.map(|t| t as usize),
                    tail_db: self.float()?,
                }
            }
            1 => Detail::Continuous { rule: self.rule()? },
            2 => Detail::Cropped {
                rule: self.rule()?,
                tail_db: self.float()?,
            },
            3 => Detail::Point {
                rule: self.rule()?,
                alias_db: self.float()?,
            },
            4 => Detail::Spectrum {
                rule: self.rule()?,
                wrap_db: f64::from_bits(self.word()?),
            },
            5 => Detail::Roundtrip {
                rule: self.rule()?,
                edited: match self.byte()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                },
            },
            6 => Detail::Reading { rule: self.rule()? },
            7 => {
                let count = self.word()? as usize;
                let parts = (0..count.min(self.0.len()))
                    .map(|_| self.detail())
                    .collect::<Option<Vec<_>>>()?;
                if parts.len() != count {
                    return None;
                }
                Detail::Added { parts }
            }
            _ => return None,
        })
    }
}
