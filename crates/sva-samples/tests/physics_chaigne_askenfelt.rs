// Concern: states what a chaigne_askenfelt solver must hold over the buffer it steps out | Non-concern: the site's own FD math (src/physics/chaigne_askenfelt.rs) | IO: (Params) -> asserted peaks

mod fd;

use sva_samples::Params;
use sva_samples::physics::chaigne_askenfelt::ChaigneAskenfeltParams;

#[test]
fn a_stepped_spectrum_matches_the_stiff_string_partial_formula() {
    let (f0, b) = (220.0, 0.01);
    let params = Params::ChaigneAskenfelt(ChaigneAskenfeltParams {
        b,
        ..ChaigneAskenfeltParams::at(f0)
    });
    let buffer = fd::render(&params, 44_100, 0.5);
    fd::sounds(&buffer);
    let peaks = fd::peaks(&buffer, 24, None);

    for n in 1..=6u32 {
        let predicted = f64::from(n) * f0 * (1.0 + b * f64::from(n * n)).sqrt();
        let found = fd::nearest(&peaks, predicted);
        let error = (found - predicted).abs() / predicted;
        assert!(
            error < 0.05,
            "partial {n}: measured {found} predicted {predicted} error {error}"
        );
    }
}

#[test]
fn a_chaigne_askenfelt_site_steps_the_same_buffer_every_time() {
    let params = Params::ChaigneAskenfelt(ChaigneAskenfeltParams {
        vel: 3.2,
        ..ChaigneAskenfeltParams::at(220.0)
    });
    fd::is_deterministic(&params, 22_050, 0.2);
}

#[test]
fn a_unison_coupled_site_steps_the_same_buffer_every_time() {
    let params = Params::ChaigneAskenfelt(ChaigneAskenfeltParams {
        vel: 3.2,
        unison_count: 2.0,
        detune: 2f64.powf(5.0 / 1200.0),
        ..ChaigneAskenfeltParams::at(220.0)
    });
    fd::is_deterministic(&params, 22_050, 0.2);
}

#[test]
fn a_parameter_out_of_range_is_refused() {
    let base = || ChaigneAskenfeltParams::at(220.0);
    fd::refuses(
        &Params::ChaigneAskenfelt(base()),
        [
            ChaigneAskenfeltParams { f0: 0.0, ..base() },
            ChaigneAskenfeltParams { b: -1.0, ..base() },
            ChaigneAskenfeltParams {
                strike_pos: 0.0,
                ..base()
            },
            ChaigneAskenfeltParams {
                strike_pos: 1.0,
                ..base()
            },
            ChaigneAskenfeltParams { vel: 0.0, ..base() },
            ChaigneAskenfeltParams {
                hammer_mass: -1.0,
                ..base()
            },
            ChaigneAskenfeltParams {
                hammer_k: 0.0,
                ..base()
            },
            ChaigneAskenfeltParams {
                hammer_p: 0.0,
                ..base()
            },
            ChaigneAskenfeltParams {
                damp_dc: -0.1,
                ..base()
            },
            ChaigneAskenfeltParams {
                unison_count: 0.0,
                ..base()
            },
            ChaigneAskenfeltParams {
                f0: f64::NAN,
                ..base()
            },
        ]
        .into_iter()
        .map(Params::ChaigneAskenfelt)
        .collect(),
    );
}

#[test]
fn an_undamped_run_stays_bounded() {
    fd::stays_bounded(
        &Params::ChaigneAskenfelt(ChaigneAskenfeltParams {
            damp_dc: 0.0,
            damp_freq: 0.0,
            ..ChaigneAskenfeltParams::at(220.0)
        }),
        44_100,
        0.5,
    );
}
