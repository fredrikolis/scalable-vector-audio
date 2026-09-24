// Concern: states what a bowed-string solver owes the buffer it steps out | Non-concern: the site's own FD/friction math (src/physics/willemsen_bilbao_serafin.rs) | IO: (Params) -> asserted peaks

mod fd;
mod partials;

use partials::{ringing_hz, stiff_partial};
use sva_samples::measure::spectrum;
use sva_samples::physics::willemsen_bilbao_serafin::WillemsenBilbaoSerafinParams;
use sva_samples::{Params, site};

fn bowed(f0: f64, b: f64, bow_force: f64, bow_pos: f64) -> Params {
    Params::WillemsenBilbaoSerafin(WillemsenBilbaoSerafinParams {
        b,
        bow_pos,
        bow_force,
        ..WillemsenBilbaoSerafinParams::at(f0)
    })
}

/// A light `bow_force` keeps bowing's own pitch-flattening off the dispersion relation.
#[test]
fn a_bowed_stepped_spectrum_matches_the_stiff_string_partial_formula() {
    let (f0, b) = (440.0, 1e-4);
    let buffer = fd::render(&bowed(f0, b, 0.5, 0.25), 44_100, 0.6);
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
fn a_willemsen_bilbao_serafin_site_steps_the_same_buffer_every_time() {
    let params = Params::WillemsenBilbaoSerafin(WillemsenBilbaoSerafinParams {
        bow_vel: 0.1,
        bow_force: 10.0,
        ..WillemsenBilbaoSerafinParams::at(440.0)
    });
    fd::is_deterministic(&params, 22_050, 0.2);
}

/// Schelleng: extreme `bow_force` locks onto an octave-doubled period, empirically.
fn dominant_period(f0: f64, rate: u32, bow_force: f64) -> (f64, usize) {
    let buffer = fd::render(&bowed(f0, 1e-4, bow_force, 0.25), rate, 0.5);
    fd::sounds(&buffer);
    let samples = buffer.plane(0);
    let sr = f64::from(rate);

    let nominal = (sr / f0).round() as usize;
    let tail = &samples[samples.len() - 20 * nominal..];
    let mean = tail.iter().sum::<f64>() / tail.len() as f64;
    let centered: Vec<f64> = tail.iter().map(|&s| s - mean).collect();
    let energy: f64 = centered.iter().map(|c| c * c).sum();
    assert!(energy > 0.0, "a silent tail tests nothing");

    (nominal / 2..nominal + nominal / 2)
        .map(|lag| {
            let corr = centered[lag..]
                .iter()
                .zip(&centered)
                .map(|(a, b)| a * b)
                .sum::<f64>()
                / energy;
            (corr, lag)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("a lag window with at least one lag")
}

#[test]
fn a_moderate_bow_force_sustains_the_nominal_period_an_extreme_one_does_not() {
    let (f0, rate) = (440.0, 44_100u32);
    let (moderate_corr, moderate_lag) = dominant_period(f0, rate, 5.0);
    let (_, extreme_lag) = dominant_period(f0, rate, 400.0);
    let nominal = (f64::from(rate) / f0).round();
    assert!(
        moderate_corr > 0.8,
        "moderate bow force should sustain periodic Helmholtz-like motion, got {moderate_corr}"
    );
    let moderate_off = (moderate_lag as f64 - nominal).abs() / nominal;
    let extreme_off = (extreme_lag as f64 - nominal).abs() / nominal;
    assert!(
        moderate_off < 0.2,
        "moderate bow force's dominant period {moderate_lag} strayed too far from {nominal}"
    );
    assert!(
        extreme_off > moderate_off + 0.2,
        "extreme bow force should lock onto a different period: moderate {moderate_lag}, \
         extreme {extreme_lag}, nominal {nominal}"
    );
}

#[test]
fn a_parameter_out_of_range_is_refused() {
    let base = || WillemsenBilbaoSerafinParams::at(440.0);
    fd::refuses(
        &Params::WillemsenBilbaoSerafin(base()),
        [
            WillemsenBilbaoSerafinParams { f0: 0.0, ..base() },
            WillemsenBilbaoSerafinParams { b: -1.0, ..base() },
            WillemsenBilbaoSerafinParams {
                bow_pos: 0.0,
                ..base()
            },
            WillemsenBilbaoSerafinParams {
                bow_pos: 1.0,
                ..base()
            },
            WillemsenBilbaoSerafinParams {
                bow_force: -1.0,
                ..base()
            },
            WillemsenBilbaoSerafinParams {
                mu_s: 0.0,
                ..base()
            },
            WillemsenBilbaoSerafinParams {
                mu_c: 0.0,
                ..base()
            },
            WillemsenBilbaoSerafinParams {
                stribeck_vel: 0.0,
                ..base()
            },
            WillemsenBilbaoSerafinParams {
                bristle_stiffness: 0.0,
                ..base()
            },
            WillemsenBilbaoSerafinParams {
                f0: f64::NAN,
                ..base()
            },
        ]
        .into_iter()
        .map(Params::WillemsenBilbaoSerafin)
        .collect(),
    );
}

/// Bowed friction pumps energy in, so the bound is the string's own saturation, not decay:
/// what the deleted Newton-Raphson sweep really guarded is that nothing diverges.
#[test]
fn a_long_bowed_run_stays_bounded() {
    fd::stays_bounded(&bowed(440.0, 1e-4, 5.0, 0.25), 44_100, 0.5);
}

/// A bow that never sticks, with no Stribeck drop and no damping in the contact, leans on the
/// string with one constant force, so the string rings free.
fn slipping(f0: f64, b: f64, bow_pos: f64) -> WillemsenBilbaoSerafinParams {
    WillemsenBilbaoSerafinParams {
        b,
        bow_pos,
        bow_vel: 1.0,
        bow_force: 1.0,
        mu_s: 0.3,
        mu_c: 0.3,
        bristle_damping: 0.0,
        viscous_friction: 0.0,
        ..WillemsenBilbaoSerafinParams::at(f0)
    }
}

/// Partials 1 and 2 imply the fundamental and inharmonicity asked for, at either rate.
#[test]
fn partials_one_and_two_imply_the_asked_f0_and_b_at_any_rate() {
    let b = 0.000_492_652_403_124_032_3;
    for rate in [44_100u32, 96_000] {
        for f0 in [65.406_4, 261.625_6, 1_046.502_3, 2_093.004_5] {
            let p = slipping(f0, b, 0.25);
            let buffer = fd::render(&Params::WillemsenBilbaoSerafin(p.clone()), rate, 1.0);
            let x = buffer.plane(0);
            let (f1, f2) = (
                ringing_hz(x, f64::from(rate), stiff_partial(p.f0, p.b, 1)),
                ringing_hz(x, f64::from(rate), stiff_partial(p.f0, p.b, 2)),
            );
            let r = (f2 / (2.0 * f1)).powi(2);
            let realized_b = (r - 1.0) / (4.0 - r);
            let cents = 1200.0 * (f1 / (1.0 + realized_b).sqrt() / f0).log2();
            assert!(
                cents.abs() < 0.001,
                "{f0} Hz at {rate} rings {cents} cents off"
            );
            assert!(
                (realized_b / b - 1.0).abs() < 1e-4,
                "{f0} Hz at {rate} rings b = {realized_b}, asked {b}"
            );
        }
    }
}

/// A partial 2 past Nyquist has no grid to ring on, so the site refuses instead of diverging.
#[test]
fn a_string_whose_second_partial_passes_nyquist_refuses() {
    let p = Params::WillemsenBilbaoSerafin(WillemsenBilbaoSerafinParams::at(12_000.0));
    let Err(refused) = site(&p, 44_100) else {
        panic!("a 12 kHz string rang at 44.1 kHz");
    };
    assert_eq!(refused.code(), "samples.string_past_rate", "{refused:?}");
}

/// The largest level within a quarter of `f0` of partial `k`.
fn partial_db(p: &WillemsenBilbaoSerafinParams, x: &[f64], rate: f64, k: u32) -> f64 {
    let (mags, bin_hz, _, _) = spectrum::magnitudes(x, rate, None);
    let hz = stiff_partial(p.f0, p.b, k);
    let band = ((hz - p.f0 / 4.0) / bin_hz) as usize..=((hz + p.f0 / 4.0) / bin_hz) as usize;
    20.0 * mags[band].iter().fold(0.0f64, |m, &v| m.max(v)).log10()
}

/// At C6 the grid holds about twenty points, so 1/7 and 1/8 once bowed the same node. Bowed
/// where written, each silences its own partial: `sin(k pi x) = 0` at `k = 1/x`.
#[test]
fn a_bow_between_nodes_notches_the_partial_its_position_names() {
    let rate = 44_100.0;
    let bowed_at = |k: u32| {
        let p = slipping(1_046.502_3, 1e-4, 1.0 / f64::from(k));
        let buffer = fd::render(&Params::WillemsenBilbaoSerafin(p.clone()), 44_100, 1.0);
        (1..=9)
            .map(|j| partial_db(&p, buffer.plane(0), rate, j))
            .collect::<Vec<f64>>()
    };
    let (at_7, at_8) = (bowed_at(7), bowed_at(8));
    for (k, notched, other) in [(7, &at_7, &at_8), (8, &at_8, &at_7)] {
        let depth = notched[k - 1] - notched[k - 2].max(notched[k]);
        assert!(depth < -60.0, "bowed at 1/{k}, partial {k} sits {depth} dB");
        let moved = notched[k - 1] - other[k - 1];
        assert!(
            moved < -60.0,
            "partial {k} bowed at 1/{k} is only {moved} dB below the other bow's"
        );
    }
}

/// A bow a thousand times faster than the reference drives the friction solve past settling:
/// the site refuses at the sample it failed rather than handing the string a force.
#[test]
fn a_friction_solve_that_does_not_settle_refuses() {
    let p = Params::WillemsenBilbaoSerafin(WillemsenBilbaoSerafinParams {
        bow_vel: 1e3,
        ..WillemsenBilbaoSerafinParams::at(440.0)
    });
    let mut solver = site(&p, 44_100).expect("a grid this rate holds");
    let refused = (0..44_100)
        .find_map(|_| solver.step().err())
        .expect("the solve fails to settle within a second");
    assert_eq!(refused.code(), "samples.contact_unsettled", "{refused}");
}
