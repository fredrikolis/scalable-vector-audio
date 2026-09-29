// Concern: states that a closed form read span by span writes the samples one whole read writes | Non-concern: which rows an extent picks by cost | IO: (a ClosedForm) -> bit-equal samples

use crate::helpers::part;
use std::f64::consts::TAU;

use sva_formula::{Body, C64, ClosedForm, Edge, Origin, Unary, Var};
use sva_samples::collapse::{self, AliasScore, Extent};
use sva_samples::{CollapseError, PSYCHOACOUSTIC_V1, Refs, Rows, Tape};

const RATE: u32 = 8_000;
const LEN: usize = 4_000;

fn form(var: Var, body: Body) -> ClosedForm {
    ClosedForm {
        var,
        body,
        origin: Origin::new(0),
    }
}

fn cosine(hz: f64, amp: f64) -> Body {
    Body::Mul(vec![
        part(Body::Const(C64::real(amp))),
        part(Body::Apply(
            Unary::Cos,
            part(Body::Mul(vec![
                part(Body::Const(C64::real(TAU * hz))),
                part(Body::Line),
            ])),
        )),
    ])
}

/// Spans of `block` samples from grid sample `start` to the end.
fn blocks(form: &ClosedForm, start: i64, block: i64) -> Vec<f64> {
    let rows = Rows::of(form, RATE, &PSYCHOACOUSTIC_V1).expect("rows");
    let mut tape = Tape::new(rows.width(), LEN, start);
    while tape.end() < LEN as i64 {
        let to = (tape.end() + block).min(LEN as i64);
        rows.extend(to, &mut tape).expect("a span");
    }
    tape.since(0, start).to_vec()
}

fn whole(form: &ClosedForm, start: i64) -> Vec<f64> {
    let extent = Extent::new(start, LEN as i64);
    let (buffer, _) =
        collapse::render(form, RATE, extent, &PSYCHOACOUSTIC_V1, AliasScore::NotAsked)
            .expect("a whole read");
    buffer.plane(0).to_vec()
}

/// Three lines are summed directly by a whole read too, and `tanh` is point-sampled there:
/// both whole rows read one instant at a time, so the spans agree with them bit for bit,
/// wherever on the grid either starts.
#[test]
fn a_form_whose_whole_row_reads_one_instant_at_a_time_is_the_same_in_any_span() {
    let chord = form(
        Var::T,
        Body::Add(vec![
            part(cosine(220.0, 0.5)),
            part(cosine(330.0, 0.25)),
            part(cosine(0.0, 0.125)),
        ]),
    );
    let shaped = form(Var::T, Body::Apply(Unary::Tanh, part(cosine(110.0, 3.0))));
    for written in [chord, shaped] {
        for start in [0, 777] {
            let want = whole(&written, start);
            assert!(want.iter().any(|v| *v != 0.0), "silence tests nothing");
            for block in [1, 128, 999, LEN as i64] {
                assert_eq!(
                    blocks(&written, start, block),
                    want,
                    "blocks of {block} from {start}"
                );
            }
        }
    }
}

#[test]
fn a_form_in_f_has_no_row_a_span_reads_alone() {
    let spectrum = form(Var::F, Body::Const(C64::real(1.0)));
    assert_eq!(
        Rows::of(&spectrum, RATE, &PSYCHOACOUSTIC_V1).err(),
        Some(CollapseError::NoBlockRow)
    );
}

struct NoNodes;

impl Refs for NoNodes {
    fn value(&self, _: sva_formula::NodeId, _: usize, _: f64) -> Result<C64, CollapseError> {
        Err(CollapseError::NotEvaluable("a node"))
    }

    fn width(&self, _: sva_formula::NodeId) -> usize {
        1
    }
}

/// A left-nested sum of shaped tones, each cropped to `width` seconds, `gap` apart.
fn notes(terms: usize, gap: f64, width: f64) -> Body {
    let note = |k: usize| Body::Shift {
        by: k as f64 * gap,
        of: part(Body::Crop {
            of: part(Body::Apply(Unary::Tanh, part(cosine(440.0, 3.0)))),
            l: Edge::at(0.0),
            r: Edge::at(width),
            rise: 0.0,
            fall: 0.0,
        }),
    };
    (1..terms).fold(note(0), |sum, k| Body::Add(vec![part(sum), part(note(k))]))
}

/// A point-sampled sum visits each term only inside its crop: the same bits, and linear work.
#[test]
fn a_point_sampled_sum_reads_each_term_only_inside_its_crop() {
    let (gap, width) = (0.05, 0.01);
    let len = |terms: usize| (terms as f64 * gap * f64::from(RATE)) as i64;
    let rows = |terms: usize| {
        Rows::of(
            &form(Var::T, notes(terms, gap, width)),
            RATE,
            &PSYCHOACOUSTIC_V1,
        )
        .expect("rows")
    };

    let written = notes(8, gap, width);
    let planes = rows(8).planes(0, len(8)).expect("samples");
    for (n, v) in planes[0].iter().enumerate() {
        let t = n as f64 * (1.0 / f64::from(RATE));
        let want = sva_samples::eval_written_at(&written, 0, t, &NoNodes).expect("a value");
        assert_eq!(v.to_bits(), want.re.to_bits(), "sample {n}");
    }

    let (few, many) = (rows(20).work(0, len(20)).0, rows(40).work(0, len(40)).0);
    assert!(
        many * 10 <= few * 21,
        "twice the terms over twice the span cost {many}, against {few}"
    );
}
