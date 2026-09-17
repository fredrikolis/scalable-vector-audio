// Concern: states what a chaigne_doutaut solver must hold over the buffer it steps out | Non-concern: the site's own FD math (src/physics/chaigne_doutaut.rs) | IO: (Params) -> asserted peaks

mod fd;

use sva_samples::Params;
use sva_samples::physics::chaigne_doutaut::ChaigneDoutautParams;

/// Pinned `frame_secs`: two rates, one bin grid.
fn spectrum_peaks(f0: f64, rate: u32, secs: f64) -> Vec<f64> {
    let params = Params::ChaigneDoutaut(ChaigneDoutautParams {
        damp_dc: 0.0,
        damp_freq: 0.0,
        ..ChaigneDoutautParams::at(f0)
    });
    let buffer = fd::render(&params, rate, secs);
    fd::sounds(&buffer);
    fd::peaks(&buffer, 24, Some(secs))
}

/// Fletcher & Rossing's free-free roots; a low `f0` buys nodes for the partials.
#[test]
fn a_stepped_spectrum_matches_the_free_free_transcendental_roots() {
    let peaks = spectrum_peaks(40.0, 44_100, 2.0);
    assert!(peaks.len() >= 2, "not enough peaks found: {peaks:?}");

    let f1 = peaks[0];
    let targets = [1.0, 2.756_508_486, 5.404_001_396, 8.932_762_58];
    let ratios: Vec<f64> = targets
        .iter()
        .map(|&t| fd::nearest(&peaks, f1 * t) / f1)
        .collect();

    // Fewer grid nodes resolve each higher partial, so no bound tightens with order.
    for (i, tolerance) in [(1, 0.01), (2, 0.02), (3, 0.03)] {
        let error = (ratios[i] - targets[i]).abs() / targets[i];
        assert!(
            error < tolerance,
            "partial {} ratio {} is {error} off the transcendental root {}",
            i + 1,
            ratios[i],
            targets[i]
        );
    }
}

#[test]
fn higher_partial_error_shrinks_with_finer_resolution() {
    let target4 = 8.932_762_58;
    let ratio4 = |peaks: &[f64]| -> f64 {
        let f1 = peaks[0];
        fd::nearest(peaks, f1 * target4) / f1
    };
    let err = |peaks: &[f64]| (ratio4(peaks) - target4).abs() / target4;
    let coarse = err(&spectrum_peaks(40.0, 44_100, 2.0));
    let fine = err(&spectrum_peaks(40.0, 176_400, 2.0));
    assert!(
        fine <= coarse * 0.7,
        "finer resolution did not shrink partial-4 error: coarse={coarse} fine={fine}"
    );
}

#[test]
fn a_chaigne_doutaut_site_steps_the_same_buffer_every_time() {
    fd::is_deterministic(
        &Params::ChaigneDoutaut(ChaigneDoutautParams::at(261.6)),
        44_100,
        0.1,
    );
}

#[test]
fn a_parameter_out_of_range_is_refused() {
    let base = || ChaigneDoutautParams::at(261.6);
    fd::refuses(
        &Params::ChaigneDoutaut(base()),
        [
            ChaigneDoutautParams { f0: 0.0, ..base() },
            ChaigneDoutautParams {
                strike_pos: 0.0,
                ..base()
            },
            ChaigneDoutautParams {
                strike_pos: 1.0,
                ..base()
            },
            ChaigneDoutautParams { vel: 0.0, ..base() },
            ChaigneDoutautParams {
                hammer_mass: -1.0,
                ..base()
            },
            ChaigneDoutautParams {
                hammer_k: 0.0,
                ..base()
            },
            ChaigneDoutautParams {
                hammer_p: 0.0,
                ..base()
            },
            ChaigneDoutautParams {
                damp_freq: -0.1,
                ..base()
            },
            ChaigneDoutautParams {
                f0: f64::INFINITY,
                ..base()
            },
        ]
        .into_iter()
        .map(Params::ChaigneDoutaut)
        .collect(),
    );
}

#[test]
fn an_undamped_run_stays_bounded() {
    fd::stays_bounded(
        &Params::ChaigneDoutaut(ChaigneDoutautParams {
            damp_dc: 0.0,
            damp_freq: 0.0,
            ..ChaigneDoutautParams::at(261.6)
        }),
        44_100,
        0.5,
    );
}
