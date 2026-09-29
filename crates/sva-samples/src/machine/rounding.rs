// Concern: bounds how far a moving reading's computed position strays from its exact one over a span | Non-concern: the kernel's own error (reconstruct/) | IO: (At, span, rate) -> samples

use super::renderer::{At, Binary, NodeRenderer, Unary};

/// One ulp of a value, over a correctly rounded op's half.
const ULP: f64 = f64::EPSILON;

/// Four ulps, over the one glibc and musl state for `sin`, `cos`, `exp`, `tanh` and `ln`.
const LIBM: f64 = 4.0 * f64::EPSILON;

const MOST_JUMPS: f64 = 1e6;

#[derive(Clone, Copy)]
struct Held {
    lo: f64,
    hi: f64,
    err: f64,
}

impl Held {
    fn most(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }

    fn padded(lo: f64, hi: f64, err: f64) -> Held {
        let pad = ULP * lo.abs().max(hi.abs());
        Held {
            lo: lo - pad,
            hi: hi + pad,
            err,
        }
    }
}

struct Span {
    from: i64,
    to: i64,
    rate: f64,
}

/// `None` where no bound holds: an op this does not follow, a divisor or root reaching zero,
/// or a jump some sample's rounding could land on the wrong side of.
pub fn position_error(at: &At, (from, to): (i64, i64), rate: u32) -> Option<f64> {
    let At::Moving { per_sec, time, .. } = at else {
        return Some(0.0);
    };
    if from >= to {
        return Some(0.0);
    }
    let span = Span {
        from,
        to,
        rate: f64::from(rate),
    };
    let held = held(time, &span)?;
    let err = held.err * per_sec + ULP * held.most() * per_sec;
    err.is_finite().then_some(err)
}

fn held(r: &NodeRenderer, span: &Span) -> Option<Held> {
    Some(match r {
        NodeRenderer::Const(c) => Held::padded(*c, *c, 0.0),
        NodeRenderer::Wrap(wrap) => {
            let most =
                wrap.most(span.from.unsigned_abs().max(span.to.unsigned_abs()) as f64 / span.rate);
            let (lo, hi) = match span.to - span.from {
                1 => {
                    let at = wrap.at(span.from, span.rate as u32)?;
                    (at, at)
                }
                _ => (-most, most),
            };
            Held::padded(lo, hi, 2.0 * ULP * most)
        }
        NodeRenderer::Time => {
            let (lo, hi) = (
                span.from as f64 / span.rate,
                (span.to - 1) as f64 / span.rate,
            );
            let exact =
                span.to == span.from + 1 && lo.mul_add(span.rate, -(span.from as f64)) == 0.0;
            let err = if exact {
                0.0
            } else {
                ULP * lo.abs().max(hi.abs())
            };
            Held::padded(lo, hi, err)
        }
        NodeRenderer::Add(parts) => {
            let mut sum = Held::padded(0.0, 0.0, 0.0);
            for p in parts {
                let h = held(p, span)?;
                sum = Held::padded(
                    sum.lo + h.lo,
                    sum.hi + h.hi,
                    sum.err + h.err + ULP * (sum.most() + h.most()),
                );
            }
            sum
        }
        NodeRenderer::Sub(a, b) => {
            let (a, b) = (held(a, span)?, held(b, span)?);
            Held::padded(
                a.lo - b.hi,
                a.hi - b.lo,
                a.err + b.err + ULP * (a.most() + b.most()),
            )
        }
        NodeRenderer::Mul(parts) => {
            let mut product = Held::padded(1.0, 1.0, 0.0);
            for p in parts {
                product = times(product, held(p, span)?);
            }
            product
        }
        NodeRenderer::Div(a, b) => {
            let (a, b) = (held(a, span)?, held(b, span)?);
            let least = match (b.lo - b.err > 0.0, b.hi + b.err < 0.0) {
                (true, _) => b.lo - b.err,
                (_, true) => -(b.hi + b.err),
                _ => return None,
            };
            let most = a.most() / least;
            Held::padded(
                -most,
                most,
                a.err / least + a.most() * b.err / (least * least) + ULP * most,
            )
        }
        NodeRenderer::Map(f, x) => unary(*f, held(x, span)?, x, span)?,
        NodeRenderer::Zip(Binary::Max | Binary::Min, a, b) => {
            let (a, b) = (held(a, span)?, held(b, span)?);
            Held::padded(a.lo.min(b.lo), a.hi.max(b.hi), a.err.max(b.err))
        }
        NodeRenderer::Zip(Binary::Mod, a, b) => {
            let NodeRenderer::Const(p) = **b else {
                return None;
            };
            let x = held(a, span)?;
            let clear = p > 0.0 && clears(a, x.err, p, 0.0, span)?;
            clear.then_some(Held::padded(0.0, p, x.err + ULP * p))?
        }
        _ => return None,
    })
}

fn times(a: Held, b: Held) -> Held {
    let ends = [a.lo * b.lo, a.lo * b.hi, a.hi * b.lo, a.hi * b.hi];
    let (lo, hi) = ends
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(*v), hi.max(*v))
        });
    let (x, y) = (a.most(), b.most());
    Held::padded(lo, hi, x * b.err + y * a.err + a.err * b.err + ULP * x * y)
}

fn unary(f: Unary, x: Held, written: &NodeRenderer, span: &Span) -> Option<Held> {
    Some(match f {
        Unary::Sin | Unary::Cos => Held::padded(-1.0, 1.0, x.err.min(2.0) + LIBM),
        Unary::Tanh => Held::padded(x.lo.tanh(), x.hi.tanh(), x.err + LIBM),
        Unary::Abs => {
            let lo = if x.lo <= 0.0 && x.hi >= 0.0 {
                0.0
            } else {
                x.lo.abs().min(x.hi.abs())
            };
            Held::padded(lo, x.most(), x.err)
        }
        Unary::Sat => Held::padded(x.lo.clamp(-1.0, 1.0), x.hi.clamp(-1.0, 1.0), x.err),
        Unary::Exp => {
            let top = (x.hi + x.err).exp();
            Held::padded(x.lo.exp(), x.hi.exp(), top * x.err.exp_m1() + LIBM * top)
        }
        Unary::Sqrt if x.lo - x.err > 0.0 => {
            let root = x.hi.sqrt();
            Held::padded(
                x.lo.sqrt(),
                root,
                x.err / (2.0 * (x.lo - x.err).sqrt()) + ULP * root,
            )
        }
        Unary::Log if x.lo - x.err > 0.0 => {
            let most = x.lo.ln().abs().max(x.hi.ln().abs());
            Held::padded(x.lo.ln(), x.hi.ln(), x.err / (x.lo - x.err) + LIBM * most)
        }
        Unary::Step if clears(written, x.err, 0.0, 1.0, span)? => Held::padded(0.0, 1.0, 0.0),
        _ => return None,
    })
}

/// Whether every sample of the span is further than `err` from where `x`, affine in `t`,
/// crosses a multiple of `period`, or crosses `at` where `period` is zero.
fn clears(x: &NodeRenderer, err: f64, period: f64, at: f64, span: &Span) -> Option<bool> {
    let (slope, offset) = affine(x)?;
    if slope == 0.0 {
        return Some(true);
    }
    let value = |n: i64| slope * (n as f64 / span.rate) + offset;
    let (a, b) = (value(span.from), value(span.to - 1));
    let (lo, hi) = (a.min(b), a.max(b));
    let crossings: Vec<f64> = match period > 0.0 {
        true => {
            let (first, last) = (((lo - err) / period).floor(), ((hi + err) / period).ceil());
            if last - first > MOST_JUMPS {
                return None;
            }
            (first as i64..=last as i64)
                .map(|m| m as f64 * period)
                .collect()
        }
        false => vec![at],
    };
    Some(crossings.into_iter().all(|edge| {
        let n = ((edge - offset) / slope * span.rate).round() as i64;
        (n - 2..=n + 2)
            .filter(|n| (span.from..span.to).contains(n))
            .all(|n| {
                let one = Span {
                    from: n,
                    to: n + 1,
                    rate: span.rate,
                };
                let Some(own) = held(x, &one) else {
                    return false;
                };
                let margin = 2.0 * own.err + 4.0 * ULP * own.most();
                own.err == 0.0 || (value(n) - edge).abs() > margin
            })
    }))
}

/// `slope t + offset` where the renderer spells one.
fn affine(r: &NodeRenderer) -> Option<(f64, f64)> {
    match r {
        NodeRenderer::Time => Some((1.0, 0.0)),
        NodeRenderer::Const(c) => Some((0.0, *c)),
        NodeRenderer::Add(parts) => parts.iter().try_fold((0.0, 0.0), |(s, o), p| {
            let (s2, o2) = affine(p)?;
            Some((s + s2, o + o2))
        }),
        NodeRenderer::Sub(a, b) => {
            let ((s1, o1), (s2, o2)) = (affine(a)?, affine(b)?);
            Some((s1 - s2, o1 - o2))
        }
        NodeRenderer::Mul(parts) => parts.iter().try_fold((0.0, 1.0), |(s, o), p| {
            let (s2, o2) = affine(p)?;
            match (s == 0.0, s2 == 0.0) {
                (_, true) => Some((s * o2, o * o2)),
                (true, false) => Some((o * s2, o * o2)),
                (false, false) => None,
            }
        }),
        _ => None,
    }
}
