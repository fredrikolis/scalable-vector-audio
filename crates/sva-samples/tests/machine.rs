// Concern: states what one node renderer computes a sample at a time, or refuses | Non-concern: building it (sva-engine lower.rs) | IO: (a NodeRenderer) -> samples or a refusal

use sva_samples::biquad::{State, design};
use sva_samples::machine::Ctx;
use sva_samples::machine::ops::Layout;
use sva_samples::{Buffer, NodeRenderer, Site, SiteId};

const RATE: u32 = 8_000;
const LEN: usize = 64;

/// A renderer refuses while it compiles, before the first sample, so no buffer is needed to
/// read that refusal back.
fn silent(renderer: &NodeRenderer, layout: &Layout) -> Result<Buffer, sva_samples::SampleError> {
    renderer.run(
        layout,
        &Ctx {
            rate: RATE,
            origin_secs: 0.0,
            len: 0,
            reads: &[],
            self_planes: &[],
            written: 0,
        },
    )
}

fn ramp() -> Buffer {
    Buffer::mono(RATE, (0..LEN).map(|i| i as f64 / LEN as f64).collect())
}

/// `lowpass(@ramp, cutoff=800, q=0.7) + 0.5*self(t - 1sp)`: a read slot, a filter call site
/// and a one-step recurrence in one renderer.
fn renderer() -> NodeRenderer {
    NodeRenderer::Add(vec![
        NodeRenderer::Filter {
            site: SiteId(0),
            x: Box::new(NodeRenderer::Buffer {
                id: sva_samples::BufId(0),
                shift: 0,
            }),
            cutoff: Box::new(NodeRenderer::Const(800.0)),
            q: Box::new(NodeRenderer::Const(0.7)),
            gain: Box::new(NodeRenderer::Const(0.0)),
        },
        NodeRenderer::Mul(vec![
            NodeRenderer::Const(0.5),
            NodeRenderer::SelfAt { steps: 1 },
        ]),
    ])
}

#[test]
fn a_buffer_read_a_filter_and_a_self_recurrence_produce_the_expected_samples() {
    let read = ramp();
    let renderer = renderer();
    let layout = Layout {
        width: 1,
        read_widths: vec![1],
        sites: vec![Site::Filter(sva_samples::Shape::Lowpass)],
    };

    // One sample at a time, feeding back what the machine already wrote.
    let mut history = vec![0.0; LEN];
    for i in 0..LEN {
        let block = renderer
            .run(
                &layout,
                &Ctx {
                    rate: RATE,
                    origin_secs: 0.0,
                    len: i + 1,
                    reads: &[&read],
                    self_planes: &history,
                    written: i,
                },
            )
            .expect("a mono run");
        history[i] = block.at(0, i);
    }

    let coeffs = design(
        sva_samples::Shape::Lowpass,
        800.0,
        0.7,
        0.0,
        f64::from(RATE),
    );
    let mut state = State::default();
    let mut want = vec![0.0; LEN];
    for i in 0..LEN {
        let filtered = state.step(&coeffs, read.at(0, i));
        want[i] = filtered + 0.5 * if i == 0 { 0.0 } else { want[i - 1] };
    }

    for i in 0..LEN {
        assert!(
            (history[i] - want[i]).abs() < 1e-12,
            "sample {i}: machine {} closed form {}",
            history[i],
            want[i]
        );
    }
    assert!(history.iter().any(|&s| s != 0.0), "silence tests nothing");
}

#[test]
fn a_channel_past_the_last_component_refuses() {
    let out = silent(
        &NodeRenderer::Channel {
            x: Box::new(NodeRenderer::Buffer {
                id: sva_samples::BufId(0),
                shift: 0,
            }),
            k: 3,
        },
        &Layout {
            width: 2,
            read_widths: vec![2],
            sites: Vec::new(),
        },
    );
    assert_eq!(
        out.err(),
        Some(sva_samples::SampleError::ChannelOutOfRange { k: 3, width: 2 })
    );
}

/// A stereo read beside a mono gain: the mono side widens, and each component keeps its own
/// value rather than reading its neighbour's slot.
#[test]
fn a_mono_operand_widens_to_its_neighbours_components_without_crossing_them() {
    let stereo = Buffer::of_planes(RATE, vec![vec![1.0; LEN], vec![-2.0; LEN]]);
    let renderer = NodeRenderer::Join(vec![
        NodeRenderer::Mul(vec![
            NodeRenderer::Channel {
                x: Box::new(NodeRenderer::Buffer {
                    id: sva_samples::BufId(0),
                    shift: 0,
                }),
                k: 1,
            },
            NodeRenderer::Const(0.25),
        ]),
        NodeRenderer::Mul(vec![
            NodeRenderer::Buffer {
                id: sva_samples::BufId(0),
                shift: 0,
            },
            NodeRenderer::Const(3.0),
        ]),
    ]);
    let layout = Layout {
        width: 3,
        read_widths: vec![2],
        sites: Vec::new(),
    };
    let out = renderer
        .run(
            &layout,
            &Ctx {
                rate: RATE,
                origin_secs: 0.0,
                len: LEN,
                reads: &[&stereo],
                self_planes: &[],
                written: 0,
            },
        )
        .expect("a three-wide run");
    assert_eq!(out.width, 3);
    assert_eq!(out.at(0, 7), -0.5);
    assert_eq!(out.at(1, 7), 3.0);
    assert_eq!(out.at(2, 7), -6.0);
}

/// The refusal the shipped entry point raises for a renderer the engine could still hand it.
#[test]
fn a_width_mismatch_leaves_compile_under_its_own_code() {
    let out = silent(
        &NodeRenderer::Add(vec![
            NodeRenderer::Buffer {
                id: sva_samples::BufId(0),
                shift: 0,
            },
            NodeRenderer::Buffer {
                id: sva_samples::BufId(1),
                shift: 0,
            },
        ]),
        &Layout {
            width: 3,
            read_widths: vec![2, 3],
            sites: Vec::new(),
        },
    );
    assert_eq!(
        out.err().as_ref().map(sva_samples::SampleError::code),
        Some("type.width_mismatch")
    );
}
