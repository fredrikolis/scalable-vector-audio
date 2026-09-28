// Concern: states that a machine run in blocks, or resumed from a held state, writes one run's samples | Non-concern: which blocks a stream asks for | IO: (a NodeRenderer) -> bit-equal samples

use sva_samples::machine::ops::Layout;
use sva_samples::physics::Params;
use sva_samples::physics::chaigne_askenfelt::ChaigneAskenfeltParams;
use sva_samples::{BufId, Buffer, Ctx, Machine, NodeRenderer, Shape, Site, SiteId, Tape, Window};

const RATE: u32 = 44_100;
const LEN: usize = 3_000;

fn string() -> Params {
    Params::ChaigneAskenfelt(ChaigneAskenfeltParams::at(440.0))
}

/// `lowpass(@ramp, cutoff=900, q=0.7) + 0.5*self(t - 3sp) + 40*string`: a read, a filter, a
/// recurrence and a solver in one renderer, the string's felt pressing from `landing` on.
fn renderer(landing: f64) -> NodeRenderer {
    let damper = NodeRenderer::Crop {
        x: Box::new(NodeRenderer::Const(0.1)),
        a: landing,
        b: f64::INFINITY,
        rise: 0.0,
        fall: 0.0,
    };
    NodeRenderer::Add(vec![
        NodeRenderer::Filter {
            site: SiteId(0),
            from: 0,
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
            NodeRenderer::Physics {
                site: SiteId(1),
                from: 0,
                args: vec![damper, NodeRenderer::Const(0.0)],
            },
        ]),
    ])
}

fn layout() -> Layout {
    Layout {
        width: 1,
        read_widths: vec![1],
        sites: vec![
            Site::Filter(Shape::Lowpass),
            Site::Physics(Box::new(string())),
        ],
    }
}

fn ramp() -> Buffer {
    Buffer::mono(RATE, (0..LEN).map(|i| (i % 97) as f64 / 97.0).collect())
}

fn whole(landing: f64) -> Vec<f64> {
    let read = ramp();
    let out = renderer(landing)
        .run(
            &layout(),
            &Ctx {
                rate: RATE,
                start: 0,
                len: LEN,
                reads: &[Window::of(&read, read.extent())],
            },
        )
        .expect("one run");
    out.plane(0).to_vec()
}

fn opened(landing: f64) -> Machine {
    Machine::open(&renderer(landing), &layout(), RATE).expect("a machine")
}

fn run(machine: &mut Machine, tape: &mut Tape, to: i64) {
    let read = ramp();
    machine
        .run_to(to, &[Window::of(&read, read.extent())], tape)
        .expect("a block");
}

#[test]
fn blocks_of_any_size_write_the_samples_one_run_writes() {
    let want = whole(f64::INFINITY);
    assert!(want.iter().any(|v| *v != 0.0), "silence tests nothing");
    for block in [1, 37, 512, LEN] {
        let mut machine = opened(f64::INFINITY);
        let mut tape = Tape::new(1, LEN, 0);
        while tape.end() < LEN as i64 {
            let to = (tape.end() + block as i64).min(LEN as i64);
            run(&mut machine, &mut tape, to);
        }
        assert_eq!(tape.since(0, 0), want.as_slice(), "blocks of {block}");
    }
}

#[test]
fn a_held_state_resumed_in_a_fresh_machine_writes_the_rest_of_the_run() {
    let want = whole(f64::INFINITY);
    let mut machine = opened(f64::INFINITY);
    let mut tape = Tape::new(1, LEN, 0);
    run(&mut machine, &mut tape, 1_234);
    let (held, mut resumed_tape) = (machine.state(), tape.clone());
    run(&mut machine, &mut tape, LEN as i64);

    let mut resumed = opened(f64::INFINITY);
    assert!(resumed.carry(&held), "the same sites");
    run(&mut resumed, &mut resumed_tape, LEN as i64);
    assert_eq!(resumed_tape.since(0, 0), want.as_slice());
    assert_eq!(tape.since(0, 0), want.as_slice());
}

/// Before the felt presses the two runs agree sample and state, so a held string's state is
/// the released one's at the landing, and the one site takes it whole.
#[test]
fn a_string_held_unreleased_and_resumed_with_a_release_is_the_released_run() {
    let release = 0.04;
    let landing = (release * f64::from(RATE)).ceil() as i64;
    let want = whole(release);
    assert_ne!(want, whole(f64::INFINITY), "the release changes nothing");
    let mut held = opened(f64::INFINITY);
    let mut tape = Tape::new(1, LEN, 0);
    run(&mut held, &mut tape, landing);

    let mut resumed = opened(release);
    assert!(resumed.carry(&held.state()), "the same sites");
    run(&mut resumed, &mut tape, LEN as i64);
    assert_eq!(tape.since(0, 0), want.as_slice());
}

#[test]
fn a_state_held_for_other_parameters_is_refused() {
    let held = opened(f64::INFINITY).state();
    let other = Layout {
        sites: vec![
            Site::Filter(Shape::Highpass),
            Site::Physics(Box::new(string())),
        ],
        ..layout()
    };
    let mut machine = Machine::open(&renderer(f64::INFINITY), &other, RATE).expect("a machine");
    assert!(!machine.carry(&held), "a highpass took a lowpass's state");
}

/// Outside what a node is nonzero over a read is zero; inside it, a sample the tape does
/// not hold is a reader past its extent, and no value answers it.
#[test]
fn a_tape_reads_zero_only_where_its_node_is_silent() {
    let mut tape = Tape::new(1, 8, 2);
    for v in 1..=6 {
        tape.push(0, f64::from(v));
    }
    tape.forget_before(6);
    assert_eq!((tape.base(), tape.end()), (6, 8));
    assert_eq!(tape.since(0, 6), &[5.0, 6.0]);
    assert_eq!(tape.window().at(0, 7), 6.0);
    assert_eq!(tape.window().at(0, 1), 0.0);
    let ends = sva_samples::Extent::new(2, 8);
    assert_eq!(tape.within(ends).at(0, 8), 0.0);
}

#[test]
#[should_panic(expected = "not held")]
fn reading_a_forgotten_sample_panics() {
    let mut tape = Tape::new(1, 8, 0);
    for v in 1..=6 {
        tape.push(0, f64::from(v));
    }
    tape.forget_before(4);
    tape.window().at(0, 3);
}

#[test]
#[should_panic(expected = "not held")]
fn reading_past_what_a_tape_wrote_panics() {
    let mut tape = Tape::new(1, 8, 0);
    tape.push(0, 1.0);
    tape.window().at(0, 1);
}
