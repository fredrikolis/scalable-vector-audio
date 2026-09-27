// Concern: states that a closed form read span by span writes the samples one whole read writes | Non-concern: which rows an extent picks by cost (collapse.rs) | IO: (a ClosedForm) -> bit-equal samples

use std::f64::consts::TAU;

use sva_formula::{Body, C64, ClosedForm, Origin, Part, Unary, Var};
use sva_samples::collapse::{self, AliasScore, Extent};
use sva_samples::{CollapseError, PSYCHOACOUSTIC_V1, Rows, Tape};

const RATE: u32 = 8_000;
const LEN: usize = 4_000;

fn part(body: Body) -> Part {
    Part::bare(body)
}

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
