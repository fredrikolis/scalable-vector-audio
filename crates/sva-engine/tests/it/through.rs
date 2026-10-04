// Concern: proves a ref read as the form it names renders the bits it did when that form was written in | Non-concern: what any one form computes | IO: (a composition) -> a digest of the samples

use crate::fixtures::graph_of;
use sva_engine::{RenderConfig, Tier, render};

const RATE: u32 = 8_000;

fn digest(files: &[(&str, &str)], target: &str, secs: f64) -> u64 {
    let g = graph_of("through", files);
    let config = RenderConfig::seconds(RATE, secs);
    let render =
        render(&g, target, config, &Tier::default()).unwrap_or_else(|e| panic!("{target}: {e}"));
    let id = render.id(target).expect("the root");
    let samples = render.output(id).expect("a buffer").plane(0).to_vec();
    assert!(samples.iter().any(|v| *v != 0.0), "silence tests nothing");
    samples.iter().fold(0xcbf2_9ce4_8422_2325u64, |held, v| {
        v.to_bits().to_le_bytes().iter().fold(held, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3)
        })
    })
}

/// The kink of a `max` two refs down splits the `exp` reading it, at every place it is read.
#[test]
fn a_kink_two_refs_down_splits_the_form_reading_it() {
    let files = [
        ("ramp", "max(t - 0.25, 0)\n"),
        ("mid", "@ramp(t)*0.5 + @ramp(t)*0.5\n"),
        ("top", "crop(exp(-3*@mid(t))*sin(2*pi*220*t), 0s, 1s)\n"),
    ];
    assert_eq!(digest(&files, "top", 1.0), 0x5b93_8aeb_39ee_b29f);
}

/// A form no atom sum reaches, read at a time that moves, is its written form at that time.
#[test]
fn a_written_form_read_at_a_moving_time_reads_each_ref_it_holds() {
    let files = [
        ("n0", "crop(sin(2*pi*220*t), 0s, 1s)\n"),
        ("n1", "max(@n0(t), 0)*0.5 + @n0(t)*0.3 + @n0(t)*0.2\n"),
        ("n2", "max(@n1(t), 0)*0.5 + @n1(t)*0.3 + @n1(t)*0.2\n"),
        ("top", "crop(@n2(t + 0.002*sin(2*pi*5*t)), 0s, 0.5s)\n"),
    ];
    assert_eq!(digest(&files, "top", 0.5), 0x9929_8012_9a93_a725);
}

#[test]
fn an_index_of_a_time_held_in_refs_reads_the_step_nearest_it() {
    let files = [
        (
            "x",
            "lowpass(crop(sample(sin(2*pi*220*t)), 0s, 0.05s), cutoff=900)\n",
        ),
        ("wobble", "0.002s*sin(2*pi*5*t)\n"),
        ("late", "t - 0.003s - @wobble(t)\n"),
        ("when", "@late(t) - 0.002s\n"),
        ("top", "@x[idx(@when(t))]\n"),
    ];
    assert_eq!(digest(&files, "top", 0.1), 0x7898_7b4c_b8f2_a322);
}

/// A dashpot ramped in through refs switches where the crop it reads opens.
#[test]
fn a_solver_argument_held_in_refs_switches_where_its_crop_opens() {
    let files = [
        ("lift", "crop(min(1, (t - 0.1)/0.03), 0.1s, inf)\n"),
        ("felt", "0.1*@lift(t)\n"),
        ("top", "chaigne_askenfelt(261.63, damper_r=@felt(t))\n"),
    ];
    assert_eq!(digest(&files, "top", 0.3), 0x4330_40d6_4d9f_d526);
}

#[test]
fn a_stored_energy_coefficient_held_in_refs_only_jumps() {
    let files = [
        ("jump", "crop(1, 0.1s, inf)\n"),
        ("spring", "1e3*@jump(t)\n"),
        ("top", "chaigne_askenfelt(261.63, damper_k=@spring(t))\n"),
    ];
    assert_eq!(digest(&files, "top", 0.3), 0x2bce_0950_14ae_d401);
}

/// A closed loop's series reads each ref its body reads as the form it names, at each term's
/// own time.
#[test]
fn a_closed_loop_reads_each_ref_its_body_reads_at_each_terms_time() {
    let files = [
        ("tone", "crop(sin(2*pi*220*t), 0s, 0.1s)\n"),
        ("pair", "@tone(t)*0.5 + @tone(t)*0.5\n"),
        ("top", "@pair(t) + 0.5*self(t - 17ms)\n"),
    ];
    assert_eq!(digest(&files, "top", 0.5), 0x451a_bb5d_2453_5915);
}

/// A series term reading a closed form at its own instant renders the bits it does with that
/// form written in.
#[test]
fn a_series_term_reading_a_form_at_its_instant_is_that_form_written_in() {
    let read = [
        ("x", "0.5 + 0.25*sin(2*pi*3*t)\n"),
        (
            "top",
            "crop(sum(k, 1, inf, @x(t)*sin(2*pi*110*k*t)/(k*k)), 0s, 0.1s)\n",
        ),
    ];
    let written = [(
        "top",
        "crop(sum(k, 1, inf, (0.5 + 0.25*sin(2*pi*3*t))*sin(2*pi*110*k*t)/(k*k)), 0s, 0.1s)\n",
    )];
    assert_eq!(digest(&read, "top", 0.1), digest(&written, "top", 0.1));
}
