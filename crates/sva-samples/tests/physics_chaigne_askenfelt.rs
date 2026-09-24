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

/// Every published equation predates the unison mechanics, so one naming none of them must
/// step out the very samples it always did: FNV-1a over the bits of one second.
#[test]
fn a_call_naming_no_unison_mechanics_renders_the_samples_it_always_did() {
    let buffer = fd::render(&Params::ChaigneAskenfelt(published_unison()), 44_100, 1.0);
    let hash = buffer
        .plane(0)
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, s| {
            s.to_bits().to_le_bytes().iter().fold(h, |h, &byte| {
                (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            })
        });
    assert_eq!(hash, 0x76fb_a801_eddd_be0c, "a published unison changed");
}

/// A partial's level over one Hann window, off the single DFT bin at `hz`.
fn level_db(x: &[f64], rate: f64, hz: f64, start: usize, len: usize) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, s) in x[start..start + len].iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / len as f64).cos();
        let phase = std::f64::consts::TAU * hz * (start + i) as f64 / rate;
        re += w * s * phase.cos();
        im -= w * s * phase.sin();
    }
    10.0 * (re * re + im * im).log10()
}

fn partial_hz(p: &ChaigneAskenfeltParams, k: u32) -> f64 {
    f64::from(k) * p.f0 * (1.0 + p.b * f64::from(k * k)).sqrt()
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
            let hz = partial_hz(p, k);
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
        let hz = partial_hz(&published, k);
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
