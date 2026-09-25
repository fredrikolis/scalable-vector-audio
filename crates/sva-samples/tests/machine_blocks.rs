// Concern: states that a machine run in blocks writes the samples one run writes | Non-concern: which blocks a stream asks for | IO: (a NodeRenderer) -> bit-equal samples

use sva_samples::machine::ops::Layout;
use sva_samples::physics::Params;
use sva_samples::physics::chaigne_askenfelt::ChaigneAskenfeltParams;
use sva_samples::{BufId, Buffer, Ctx, Machine, NodeRenderer, Shape, Site, SiteId, Tape, Window};

const RATE: u32 = 44_100;
const LEN: usize = 3_000;

fn string(release: f64) -> Params {
    Params::ChaigneAskenfelt(ChaigneAskenfeltParams {
        release,
        ..ChaigneAskenfeltParams::at(440.0)
    })
}

/// `lowpass(@ramp, cutoff=900, q=0.7) + 0.5*self(t - 3sp) + 40*string`: a read, a filter, a
/// recurrence and a solver in one renderer.
fn renderer() -> NodeRenderer {
    NodeRenderer::Add(vec![
        NodeRenderer::Filter {
            site: SiteId(0),
            x: Box::new(NodeRenderer::Buffer {
                id: BufId(0),
                shift: -2,
            }),
            cutoff: Box::new(NodeRenderer::Const(900.0)),
            q: Box::new(NodeRenderer::Const(0.7)),
            gain: Box::new(NodeRenderer::Const(0.0)),
        },
        NodeRenderer::Mul(vec![
            NodeRenderer::Const(0.5),
            NodeRenderer::SelfAt { steps: 3 },
        ]),
        NodeRenderer::Mul(vec![
            NodeRenderer::Const(40.0),
            NodeRenderer::Physics { site: SiteId(1) },
        ]),
    ])
}

fn layout(release: f64) -> Layout {
    Layout {
        width: 1,
        read_widths: vec![1],
        sites: vec![
            Site::Filter(Shape::Lowpass),
            Site::Physics(Box::new(string(release))),
        ],
    }
}

fn ramp() -> Buffer {
    Buffer::mono(RATE, (0..LEN).map(|i| (i % 97) as f64 / 97.0).collect())
}

fn whole(release: f64) -> Vec<f64> {
    let read = ramp();
    let out = renderer()
        .run(
            &layout(release),
            &Ctx {
                rate: RATE,
                origin_secs: 0.0,
                len: LEN,
                reads: &[Window::of(&read)],
                self_planes: &[],
                written: 0,
            },
        )
        .expect("one run");
    out.plane(0).to_vec()
}

fn opened(release: f64) -> Machine {
    Machine::open(&renderer(), &layout(release), RATE, 0.0).expect("a machine")
}

fn run(machine: &mut Machine, tape: &mut Tape, to: usize) {
    let read = ramp();
    machine
        .run_to(to, &[Window::of(&read)], tape)
        .expect("a block");
}

#[test]
fn blocks_of_any_size_write_the_samples_one_run_writes() {
    let want = whole(f64::INFINITY);
    assert!(want.iter().any(|v| *v != 0.0), "silence tests nothing");
    for block in [1, 37, 512, LEN] {
        let mut machine = opened(f64::INFINITY);
        let mut tape = Tape::new(1, LEN);
        while tape.end() < LEN {
            let to = (tape.end() + block).min(LEN);
            run(&mut machine, &mut tape, to);
        }
        assert_eq!(tape.since(0, 0), want.as_slice(), "blocks of {block}");
    }
}

#[test]
fn a_tape_forgets_what_lies_before_a_sample_and_reads_zero_off_the_grid() {
    let mut tape = Tape::new(1, 8);
    for v in 1..=6 {
        tape.push(0, f64::from(v));
    }
    tape.forget_before(4);
    assert_eq!((tape.base(), tape.end()), (4, 6));
    assert_eq!(tape.since(0, 4), &[5.0, 6.0]);
    assert_eq!(tape.window().at(0, 5), 6.0);
    assert_eq!(tape.window().at(0, -1), 0.0);
    assert_eq!(tape.window().at(0, 6), 0.0);
}

#[test]
#[should_panic(expected = "forgotten")]
fn reading_a_forgotten_sample_panics() {
    let mut tape = Tape::new(1, 8);
    for v in 1..=6 {
        tape.push(0, f64::from(v));
    }
    tape.forget_before(4);
    tape.window().at(0, 3);
}
