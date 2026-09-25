// Concern: states what a chaigne_askenfelt solver must hold over the buffer it steps out | Non-concern: the site's own FD math (src/physics/chaigne_askenfelt.rs) | IO: (Params) -> asserted peaks

mod fd;
mod partials;

use partials::{level_db, ringing_hz, stiff_partial};
use sva_samples::measure::spectrum;
use sva_samples::physics::chaigne_askenfelt::ChaigneAskenfeltParams;
use sva_samples::{Params, site};

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
            ChaigneAskenfeltParams {
                bridge_mass: -1.0,
                ..base()
            },
            ChaigneAskenfeltParams {
                string_cents: [0.0, f64::INFINITY, 0.0],
                ..base()
            },
            ChaigneAskenfeltParams {
                string_hammer_k_ratio: [1.0, 1.0, 0.0],
                ..base()
            },
        ]
        .into_iter()
        .map(Params::ChaigneAskenfelt)
        .collect(),
    );
}

/// A partial 2 past Nyquist has no grid to ring on, so the site refuses instead of diverging.
#[test]
fn a_string_whose_second_partial_passes_nyquist_refuses() {
    let p = Params::ChaigneAskenfelt(ChaigneAskenfeltParams::at(12_000.0));
    let Err(refused) = site(&p, 44_100) else {
        panic!("a 12 kHz string rang at 44.1 kHz");
    };
    assert_eq!(refused.code(), "samples.string_past_rate", "{refused:?}");
}

/// A light bridge barely damped under stiff, lossy strings: each string alone is stable, but
/// the bridge row leaves the unison's mass form indefinite.
#[test]
fn a_unison_its_bridge_leaves_unstable_refuses() {
    let strings = ChaigneAskenfeltParams {
        b: 1e-4,
        damp_freq: 1e-4,
        bridge_coupling: 0.1,
        bridge_mass: 0.0,
        ..ChaigneAskenfeltParams::at(55.0)
    };
    assert!(site(&Params::ChaigneAskenfelt(strings.clone()), 44_100).is_ok());
    let unison = ChaigneAskenfeltParams {
        unison_count: 3.0,
        ..strings
    };
    let Err(refused) = site(&Params::ChaigneAskenfelt(unison), 44_100) else {
        panic!("an unstable unison stepped");
    };
    assert_eq!(refused.code(), "samples.bridge_unstable", "{refused:?}");
    for stable in [published_unison(), weinreich_unison()] {
        assert!(site(&Params::ChaigneAskenfelt(stable), 44_100).is_ok());
    }
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

/// A three-string unison as a published equation writes one at middle C, beating to nulls.
fn published_unison() -> ChaigneAskenfeltParams {
    ChaigneAskenfeltParams {
        b: 0.000_492_652_403_124_032_3,
        strike_pos: 0.088_514_488_958_233_88,
        vel: 4.543_307_086_614_173,
        hammer_mass: 0.006_412_505_482_227_443,
        hammer_k: 384_167_121.960_794_87,
        hammer_p: 2.399_554_894_193_862_7,
        damp_dc: 0.885_018_811_046_045_9,
        damp_freq: 0.000_441_094_138_384_720_24,
        unison_count: 3.0,
        detune: 1.001_465_045_903_355_8,
        bridge_coupling: 2_603.290_393_684_533_2,
        ..ChaigneAskenfeltParams::at(261.6256)
    }
}

/// The same strings on a massive bridge, each tuned and struck a little differently.
fn weinreich_unison() -> ChaigneAskenfeltParams {
    ChaigneAskenfeltParams {
        detune: 1.0,
        bridge_coupling: 100.0,
        bridge_mass: 1.0,
        string_cents: [-0.35, 0.1, 0.5],
        string_hammer_k_ratio: [1.0, 0.8, 0.6],
        ..published_unison()
    }
}

/// A published unison names none of the unison mechanics, so adding them must leave its
/// samples alone: FNV-1a over the bits of one second.
#[test]
fn a_call_naming_no_unison_mechanics_renders_its_frozen_samples() {
    let buffer = fd::render(&Params::ChaigneAskenfelt(published_unison()), 44_100, 1.0);
    let hash = buffer
        .plane(0)
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, s| {
            s.to_bits().to_le_bytes().iter().fold(h, |h, &byte| {
                (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            })
        });
    assert_eq!(hash, 0xb623_90c3_8829_fe06, "a published unison changed");
}

/// The 5-95 percentile spread of a partial's level about its own straight-line decay, over
/// the sustain from 0.3 s: a unison beating to nulls spreads by tens of dB.
fn beat_depth_db(x: &[f64], rate: f64, hz: f64) -> f64 {
    let (len, hop) = ((0.1 * rate) as usize, (0.02 * rate) as usize);
    let db: Vec<f64> = ((0.3 * rate) as usize..x.len() - len)
        .step_by(hop)
        .map(|s| level_db(x, rate, hz, s, len))
        .collect();
    let n = db.len() as f64;
    let mid = (n - 1.0) / 2.0;
    let mean = db.iter().sum::<f64>() / n;
    let slope = db
        .iter()
        .enumerate()
        .map(|(i, d)| (i as f64 - mid) * (d - mean))
        .sum::<f64>()
        / (0..db.len()).map(|i| (i as f64 - mid).powi(2)).sum::<f64>();
    let mut residual: Vec<f64> = db
        .iter()
        .enumerate()
        .map(|(i, d)| d - mean - slope * (i as f64 - mid))
        .collect();
    residual.sort_by(f64::total_cmp);
    residual[residual.len() * 95 / 100] - residual[residual.len() * 5 / 100]
}

/// Over the first twelve partials, weighted by the power each holds early in the sustain.
fn sustain_beat_depth_db(p: &ChaigneAskenfeltParams, x: &[f64], rate: f64) -> f64 {
    let (at, len) = ((0.5 * rate) as usize, (0.1 * rate) as usize);
    let mut rows: Vec<(f64, f64)> = (1..=12)
        .map(|k| {
            let hz = stiff_partial(p.f0, p.b, k);
            (
                beat_depth_db(x, rate, hz),
                10f64.powf(level_db(x, rate, hz, at, len) / 10.0),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = rows.iter().map(|r| r.1).sum::<f64>() / 2.0;
    let mut held = 0.0;
    rows.iter()
        .find(|r| {
            held += r.1;
            held >= half
        })
        .expect("a weighted median")
        .0
}

/// Weinreich's coupling: a bridge with mass splits the unison's modes instead of letting three
/// equal lines cancel, so the sustain stops beating to nulls while the strike stays the same.
#[test]
fn a_massive_bridge_and_unequal_strings_keep_a_unison_from_beating_to_nulls() {
    let rate = 44_100.0;
    let (published, weinreich) = (published_unison(), weinreich_unison());
    let a = fd::render(&Params::ChaigneAskenfelt(published.clone()), 44_100, 3.0);
    let b = fd::render(&Params::ChaigneAskenfelt(weinreich.clone()), 44_100, 3.0);
    let (beating, coupled) = (
        sustain_beat_depth_db(&published, a.plane(0), rate),
        sustain_beat_depth_db(&weinreich, b.plane(0), rate),
    );
    assert!(
        coupled * 4.0 < beating,
        "the coupled unison beats {coupled} dB deep against the published {beating} dB"
    );

    let onset = (0.05 * rate) as usize;
    let (mut drift, mut power) = (0.0, 0.0);
    for k in 1..=10 {
        let hz = stiff_partial(published.f0, published.b, k);
        let (was, now) = (
            level_db(a.plane(0), rate, hz, 0, onset),
            level_db(b.plane(0), rate, hz, 0, onset),
        );
        let weight = 10f64.powf(was / 10.0);
        drift += weight * (was - now).abs();
        power += weight;
    }
    assert!(
        drift / power < 1.0,
        "the strike's first 50 ms moved {} dB",
        drift / power
    );
}

/// The grid's own dispersion is inverted, so partials 1 and 2 ring on `k f0 sqrt(1 + b k^2)`:
/// the fundamental and inharmonicity they imply are the ones asked for, at either rate.
#[test]
fn partials_one_and_two_imply_the_asked_f0_and_b_at_any_rate() {
    let b = 0.000_492_652_403_124_032_3;
    for rate in [44_100u32, 96_000] {
        for f0 in [65.406_4, 261.625_6, 1_046.502_3, 2_093.004_5] {
            let p = ChaigneAskenfeltParams {
                b,
                ..ChaigneAskenfeltParams::at(f0)
            };
            let buffer = fd::render(&Params::ChaigneAskenfelt(p.clone()), rate, 1.0);
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

/// The largest level within a quarter of `f0` of partial `k`.
fn partial_db(p: &ChaigneAskenfeltParams, x: &[f64], rate: f64, k: u32) -> f64 {
    let (mags, bin_hz, _, _) = spectrum::magnitudes(x, rate, None);
    let hz = stiff_partial(p.f0, p.b, k);
    let band = ((hz - p.f0 / 4.0) / bin_hz) as usize..=((hz + p.f0 / 4.0) / bin_hz) as usize;
    20.0 * mags[band].iter().fold(0.0f64, |m, &v| m.max(v)).log10()
}

/// At C6 the grid holds about twenty points, so 1/7 and 1/8 once struck the same node. Struck
/// where written, each silences its own partial: `sin(k pi x) = 0` at `k = 1/x`.
#[test]
fn a_strike_between_nodes_notches_the_partial_its_position_names() {
    let rate = 44_100.0;
    let struck = |k: u32| {
        let p = ChaigneAskenfeltParams {
            strike_pos: 1.0 / f64::from(k),
            ..ChaigneAskenfeltParams::at(1_046.502_3)
        };
        let buffer = fd::render(&Params::ChaigneAskenfelt(p.clone()), 44_100, 1.0);
        (1..=9)
            .map(|j| partial_db(&p, buffer.plane(0), rate, j))
            .collect::<Vec<f64>>()
    };
    let (at_7, at_8) = (struck(7), struck(8));
    for (k, notched, other) in [(7, &at_7, &at_8), (8, &at_8, &at_7)] {
        let depth = notched[k - 1] - notched[k - 2].max(notched[k]);
        assert!(
            depth < -60.0,
            "struck at 1/{k}, partial {k} sits {depth} dB"
        );
        let moved = notched[k - 1] - other[k - 1];
        assert!(
            moved < -60.0,
            "partial {k} struck at 1/{k} is only {moved} dB below the other strike's"
        );
    }
}
