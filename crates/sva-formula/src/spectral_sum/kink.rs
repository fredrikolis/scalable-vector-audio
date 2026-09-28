// Concern: splits a form at the crossing of a min or max of two lines | Non-concern: lowering either half (build.rs) | IO: (&Body) -> the same value as two crops, or nothing

use crate::affine::exact_affine;
use crate::closed_form::{Body, Edge, Fold, Part, map_children};

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

fn crossing(f: &Body) -> Option<Crossing> {
    let Body::Fold(op @ (Fold::Min | Fold::Max), args) = f else {
        return None;
    };
    let [p, q] = args.as_slice() else {
        return None;
    };
    let (a1, b1) = exact_affine(&p.body)?;
    let (a2, b2) = exact_affine(&q.body)?;
    if ![a1, b1, a2, b2].iter().all(|c| c.is_real()) || a1.re == a2.re {
        return None;
    }
    let at = (b2.re - b1.re) / (a1.re - a2.re);
    if !at.is_finite() {
        return None;
    }
    let p_below = (a1.re > a2.re) == (*op == Fold::Min);
    let (below, above) = if p_below { (p, q) } else { (q, p) };
    Some((at, (*below.body).clone(), (*above.body).clone()))
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
