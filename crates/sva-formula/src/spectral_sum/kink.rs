// Concern: splits a form at the crossing of a min or max of two lines | Non-concern: lowering either half (build.rs) | IO: (&Body) -> the same value as two crops, or nothing

use crate::affine::exact_affine;
use crate::closed_form::{Body, Edge, Fold, Part, map_children};
use crate::complex::C64;

pub fn split(f: &Body) -> Option<Body> {
    let (kink, (at, below, above)) = first(f)?;
    let half = |with: &Body, l: Edge, r: Edge| {
        Part::bare(Body::Crop {
            of: Part::bare(replaced(f, kink, with)),
            l,
            r,
            rise: 0.0,
            fall: 0.0,
        })
    };
    Some(Body::Add(vec![
        half(&below, Edge::NegInf, Edge::at(at)),
        half(&above, Edge::at(at), Edge::PosInf),
    ]))
}

/// A constant, or a slope and the instant it is zero at: `k*(t - r)` keeps `r` itself. Each
/// fold of a written constant rounds once, at most 2^-53 of its result, as `exact_affine`.
#[derive(Clone, Copy)]
enum Line {
    Flat(f64),
    Sloped { slope: f64, root: f64 },
}

impl Line {
    fn slope(self) -> f64 {
        match self {
            Line::Flat(_) => 0.0,
            Line::Sloped { slope, .. } => slope,
        }
    }

    fn scaled(self, k: f64, divide: bool) -> Line {
        let by = |v: f64| if divide { v / k } else { v * k };
        match self {
            Line::Flat(v) => Line::Flat(by(v)),
            Line::Sloped { slope, root } => Line::Sloped {
                slope: by(slope),
                root,
            },
        }
    }

    fn plus(self, other: Line) -> Line {
        match (self, other) {
            (Line::Flat(a), Line::Flat(b)) => Line::Flat(a + b),
            (Line::Flat(c), Line::Sloped { slope, root })
            | (Line::Sloped { slope, root }, Line::Flat(c)) => match c == 0.0 {
                true => Line::Sloped { slope, root },
                false => Line::Sloped {
                    slope,
                    root: root - c / slope,
                },
            },
            (Line::Sloped { slope: s, root: r }, Line::Sloped { slope: u, root: q }) => {
                match (s + u, r == q) {
                    (0.0, _) => Line::Flat(-(s * r + u * q)),
                    (sum, true) => Line::Sloped {
                        slope: sum,
                        root: r,
                    },
                    (sum, false) => Line::Sloped {
                        slope: sum,
                        root: (s * r + u * q) / sum,
                    },
                }
            }
        }
    }
}

fn line(f: &Body) -> Option<Line> {
    let real = |c: C64| c.is_real().then_some(c.re);
    let read = match f {
        Body::Const(c) => Some(Line::Flat(real(*c)?)),
        Body::Line => Some(Line::Sloped {
            slope: 1.0,
            root: 0.0,
        }),
        Body::Shift { by, of } => match line(&of.body)? {
            Line::Sloped { slope, root } => Some(Line::Sloped {
                slope,
                root: root + by,
            }),
            flat => Some(flat),
        },
        Body::Add(parts) => parts
            .iter()
            .try_fold(Line::Flat(0.0), |acc, p| Some(acc.plus(line(&p.body)?))),
        Body::Mul(parts) => {
            let mut sloped: Option<Line> = None;
            let mut k = 1.0;
            for p in parts {
                match line(&p.body)? {
                    Line::Flat(v) => k *= v,
                    held if sloped.is_none() => sloped = Some(held),
                    _ => return None,
                }
            }
            Some(sloped.map_or(Line::Flat(k), |l| l.scaled(k, false)))
        }
        Body::Div(num, den) => match line(&den.body)? {
            Line::Flat(k) if k != 0.0 => Some(line(&num.body)?.scaled(k, true)),
            _ => None,
        },
        _ => None,
    };
    let read = read.or_else(|| {
        let (a, b) = exact_affine(f)?;
        match (real(a)?, real(b)?) {
            (0.0, b) => Some(Line::Flat(b)),
            (a, b) => Some(Line::Sloped {
                slope: a,
                root: -b / a,
            }),
        }
    })?;
    match read {
        Line::Sloped { slope: 0.0, .. } => Some(Line::Flat(0.0)),
        held => Some(held),
    }
    .filter(|l| match *l {
        Line::Flat(v) => v.is_finite(),
        Line::Sloped { slope, root } => slope.is_finite() && root.is_finite(),
    })
}

/// Each half is its own line, read from its root. Against a constant, the crossing is the
/// first double where the line as evaluated passes it, so a `max` never falls under nor a
/// `min` climbs over it, and a line meeting zero crosses at its root.
fn crossing(f: &Body) -> Option<Crossing> {
    let Body::Fold(op @ (Fold::Min | Fold::Max), args) = f else {
        return None;
    };
    let [p, q] = args.as_slice() else {
        return None;
    };
    let (l1, l2) = (line(&p.body)?, line(&q.body)?);
    if l1.slope() == l2.slope() {
        return None;
    }
    let max = *op == Fold::Max;
    match (l1, l2) {
        (Line::Flat(c), sloped @ Line::Sloped { slope, root })
        | (sloped @ Line::Sloped { slope, root }, Line::Flat(c)) => {
            let taken = |t: f64| {
                let v = slope * (t - root);
                if max { v >= c } else { v <= c }
            };
            let flat = Body::Const(C64::real(c));
            let estimate = root + c / slope;
            match (slope > 0.0) == max {
                true => Some((first_double(taken, estimate)?, flat, written(sloped))),
                false => Some((
                    first_double(|t| !taken(t), estimate)?,
                    written(sloped),
                    flat,
                )),
            }
        }
        (Line::Sloped { slope: s, root: r }, Line::Sloped { slope: u, root: v }) => {
            let at = match r == v {
                true => r,
                false => (s * r - u * v) / (s - u),
            };
            let p_below = (s > u) != max;
            let (below, above) = if p_below { (l1, l2) } else { (l2, l1) };
            at.is_finite().then(|| (at, written(below), written(above)))
        }
        (Line::Flat(_), Line::Flat(_)) => None,
    }
}

fn written(l: Line) -> Body {
    match l {
        Line::Flat(c) => Body::Const(C64::real(c)),
        Line::Sloped { slope, root } => Body::Mul(vec![
            Part::bare(Body::Const(C64::real(slope))),
            Part::bare(Body::Shift {
                by: root,
                of: Part::bare(Body::Line),
            }),
        ]),
    }
}

/// Doubles in numeric order as integers.
fn ordered(x: i64) -> i64 {
    x ^ (((x >> 63) as u64) >> 1) as i64
}

fn key(x: f64) -> i64 {
    ordered(x.to_bits() as i64)
}

fn double(k: i64) -> f64 {
    f64::from_bits(ordered(k) as u64)
}

/// The least finite double where `holds`, monotone over the doubles, turns true, galloping
/// out from `near` and bisecting; nothing where it never turns within the finite doubles.
fn first_double(holds: impl Fn(f64) -> bool, near: f64) -> Option<f64> {
    let finite = |k: i64| double(k).is_finite().then_some(k);
    let near = finite(key(near))?;
    let mut step = 1i64;
    let (mut no, mut yes) = (near, near);
    match holds(double(near)) {
        true => loop {
            no = finite(yes.checked_sub(step)?)?;
            if !holds(double(no)) {
                break;
            }
            yes = no;
            step = step.checked_mul(2)?;
        },
        false => loop {
            yes = finite(no.checked_add(step)?)?;
            if holds(double(yes)) {
                break;
            }
            no = yes;
            step = step.checked_mul(2)?;
        },
    }
    while yes - no > 1 {
        let mid = no + (yes - no) / 2;
        match holds(double(mid)) {
            true => yes = mid,
            false => no = mid,
        }
    }
    Some(double(yes))
}

/// A shift, warp or series moves the kink; a derivative would add a delta at the seam.
fn in_time(f: &Body) -> bool {
    matches!(
        f,
        Body::Add(_)
            | Body::Mul(_)
            | Body::Div(..)
            | Body::Pow(..)
            | Body::Apply(..)
            | Body::Fold(..)
            | Body::Crop { .. }
            | Body::Join(_)
            | Body::Channel(..)
    )
}

type Crossing = (f64, Body, Body);

fn first(f: &Body) -> Option<(&Body, Crossing)> {
    if let Some(found) = crossing(f) {
        return Some((f, found));
    }
    if !in_time(f) {
        return None;
    }
    crate::closed_form::children(f)
        .into_iter()
        .find_map(|p| first(&p.body))
}

fn replaced(f: &Body, kink: &Body, with: &Body) -> Body {
    if f == kink {
        return with.clone();
    }
    if !in_time(f) {
        return f.clone();
    }
    map_children(f, |p| Part::new(p.origin, replaced(&p.body, kink, with)))
}
