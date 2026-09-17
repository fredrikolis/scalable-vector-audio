// Concern: states what a botteldooren solver must hold over the buffer it steps out | Non-concern: the site's own FD math (src/physics/botteldooren.rs) | IO: (Params) -> asserted peaks

mod fd;

use sva_samples::Params;
use sva_samples::physics::botteldooren::{
    BotteldoorenParams, MIN_AXIS_UNROUNDED_NODES, NODE_COUNT_CEILING, max_axis_unrounded_nodes,
    node_count,
};

/// Blind spot: the aspect windows admit a neighbouring mixed mode.
#[test]
fn a_stepped_spectrum_matches_the_rigid_cavity_axial_modes() {
    let (f0, aspect_y, aspect_z) = (300.0, 0.75, 0.6);
    let secs = 1.0;
    let params = Params::Botteldooren(BotteldoorenParams {
        aspect_y,
        aspect_z,
        damp_dc: 0.0,
        damp_freq: 0.0,
        ..BotteldoorenParams::at(f0)
    });
    let buffer = fd::render(&params, 16_000, secs);
    fd::sounds(&buffer);
    let peaks = fd::peaks(&buffer, 32, Some(secs));

    let within = |label: &str, predicted: f64, tolerance: f64| {
        let found = fd::nearest(&peaks, predicted);
        let error = (found - predicted).abs() / predicted;
        assert!(
            error < tolerance,
            "{label}: no peak within {tolerance} of {predicted:.1}Hz; nearest was {found:.1}Hz \
             ({error:.4}); peaks: {peaks:?}"
        );
        found
    };

    // Half a grid node of slack per axis; a few percent at this grid size.
    let quantization = 0.06;
    let fundamental = within("(1,0,0) against the f0 asked for", f0, quantization);
    within("(2,0,0)", 2.0 * fundamental, 0.02);
    within("(3,0,0)", 3.0 * fundamental, 0.02);
    within("(0,1,0)", fundamental / aspect_y, quantization);
    within("(0,0,1)", fundamental / aspect_z, quantization);
}

#[test]
fn a_botteldooren_site_steps_the_same_buffer_every_time() {
    fd::is_deterministic(
        &Params::Botteldooren(BotteldoorenParams::at(600.0)),
        22_050,
        0.05,
    );
}

#[test]
fn a_parameter_out_of_range_is_refused() {
    let base = || BotteldoorenParams::at(300.0);
    fd::refuses(
        &Params::Botteldooren(base()),
        [
            BotteldoorenParams { f0: 0.0, ..base() },
            BotteldoorenParams {
                aspect_y: 0.0,
                ..base()
            },
            BotteldoorenParams {
                aspect_z: 0.0,
                ..base()
            },
            BotteldoorenParams {
                listener_x: 0.0,
                ..base()
            },
            BotteldoorenParams {
                listener_y: 1.0,
                ..base()
            },
            BotteldoorenParams {
                pulse_width: 0.0,
                ..base()
            },
            BotteldoorenParams {
                damp_dc: -0.1,
                ..base()
            },
            BotteldoorenParams {
                f0: f64::NAN,
                ..base()
            },
        ]
        .into_iter()
        .map(Params::Botteldooren)
        .collect(),
    );
}

/// `Lx = c/(2 f0)` sizing makes a 40 Hz room ~13M nodes, and a high `f0` pins every axis at
/// the node floor at once: two guards `valid()` cannot state, both needing the rate.
#[test]
fn the_node_count_guards_catch_both_ends_of_the_f0_range() {
    let sr = 16_000.0;
    assert!(node_count(40.0, 0.75, 0.6, sr) > NODE_COUNT_CEILING as f64);
    assert!(node_count(250.0, 0.75, 0.6, sr) < NODE_COUNT_CEILING as f64);
    assert!(max_axis_unrounded_nodes(1500.0, 0.75, 0.6, sr) < MIN_AXIS_UNROUNDED_NODES);
    assert!(max_axis_unrounded_nodes(250.0, 0.75, 0.6, sr) > MIN_AXIS_UNROUNDED_NODES);
}

#[test]
fn an_undamped_run_stays_bounded() {
    fd::stays_bounded(
        &Params::Botteldooren(BotteldoorenParams {
            damp_dc: 0.0,
            damp_freq: 0.0,
            ..BotteldoorenParams::at(300.0)
        }),
        16_000,
        0.5,
    );
}
