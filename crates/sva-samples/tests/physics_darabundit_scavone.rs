// Concern: states what a darabundit_scavone solver owes the buffer it steps out | Non-concern: the site's own FD math (src/physics/darabundit_scavone.rs) | IO: (Params) -> asserted peaks

mod fd;

use sva_samples::Params;
use sva_samples::physics::darabundit_scavone::{BoreParams, MAX_HOLES, ToneholeSpec};

const C0_M_S: f64 = 347.23;

fn holes(specs: &[(f64, bool, f64)]) -> [Option<ToneholeSpec>; MAX_HOLES] {
    let mut out = [None; MAX_HOLES];
    for (slot, &(pos, open, radius)) in out.iter_mut().zip(specs) {
        *slot = Some(ToneholeSpec {
            pos,
            open,
            radius,
            height: 1.1 * radius,
        });
    }
    out
}

/// The stubby bore spends 10.7% of its corrected length on the end correction, so that term
/// is under test; `(k_3.a)^2 ~ 2` there leaves no clean third peak, so the slender bore
/// covers mode 3.
#[test]
fn a_stepped_spectrum_matches_the_closed_open_quarter_wave_formula() {
    for (case, length, radius, modes) in [("stubby", 0.20, 0.04, 2u32), ("slender", 0.5, 0.01, 3)] {
        let params = Params::DarabunditScavone(BoreParams {
            radius_in: radius,
            radius_out: radius,
            pulse_width: 0.0005,
            damp_dc: 0.0,
            damp_freq: 0.0,
            ..BoreParams::at(length)
        });
        let buffer = fd::render(&params, 44_100, 0.5);
        fd::sounds(&buffer);
        let peaks = fd::peaks(&buffer, 24, None);

        let corrected_length = length + 0.6 * radius;
        for n in 1..=modes {
            let predicted = f64::from(2 * n - 1) * C0_M_S / (4.0 * corrected_length);
            let found = fd::nearest(&peaks, predicted);
            let error = (found - predicted).abs() / predicted;
            assert!(
                error < 0.05,
                "{case} mode {n}: measured {found} predicted {predicted} error {error}"
            );
        }
    }
}

#[test]
fn a_darabundit_scavone_site_steps_the_same_buffer_every_time() {
    fd::is_deterministic(&Params::DarabunditScavone(BoreParams::at(0.4)), 22_050, 0.2);
}

/// Scavone & Smith (ISMA-97) Fig. 4/6 on far-end pressure: the same causal shape.
#[test]
fn a_six_hole_fingering_transit_trace_shows_the_isma97_qualitative_shape() {
    let rate = 11_025u32;
    let params = Params::DarabunditScavone(BoreParams {
        radius_in: 0.0095,
        radius_out: 0.0095,
        excite_pos: 0.02,
        pulse_amp: 1.0,
        pulse_width: 0.0002,
        holes: holes(&[
            (0.15, false, 0.004),
            (0.25, false, 0.004),
            (0.35, false, 0.004),
            (0.50, true, 0.004),
            (0.65, true, 0.004),
            (0.80, true, 0.004),
        ]),
        ..BoreParams::at(0.6)
    });
    let buffer = fd::render(&params, rate, 0.01);
    fd::sounds(&buffer);
    let buf = buffer.plane(0);
    let sr = f64::from(rate);

    let quiet_n = (1.7 / 1000.0 * sr) as usize;
    assert!(
        buf[..quiet_n].iter().all(|&s| s == 0.0),
        "no energy should reach the far end before the direct wavefront's transit time"
    );

    let (peak_i, &peak) = buf
        .iter()
        .enumerate()
        .max_by(|a, c| a.1.abs().total_cmp(&c.1.abs()))
        .expect("a non-empty buffer");
    let peak_ms = peak_i as f64 / sr * 1000.0;
    assert!(
        (1.5..6.0).contains(&peak_ms),
        "the dominant feature should land well inside the window: {peak_ms}ms"
    );

    let tail_max = buf[(8.0 / 1000.0 * sr) as usize..]
        .iter()
        .fold(0.0f64, |m, &s| m.max(s.abs()));
    assert!(
        tail_max < 0.5 * peak.abs(),
        "ringing should have decayed to under half the peak by the last ~2ms: {tail_max} vs {peak}"
    );

    let sign_changes = buf[quiet_n..]
        .windows(2)
        .filter(|w| w[0] * w[1] < 0.0)
        .count();
    assert!(
        sign_changes > 10,
        "the response should ring with several sign changes: {sign_changes}"
    );
}

/// eq. 118's `C_c` moves a closed hole ~4.93% of peak at `t_h=1.1b`; 10% still catches a bug.
#[test]
fn several_closed_holes_step_close_to_but_not_exactly_the_plain_bore() {
    let plain = Params::DarabunditScavone(BoreParams::at(0.4));
    let closed = Params::DarabunditScavone(BoreParams {
        holes: holes(&[
            (0.2, false, 0.003),
            (0.5, false, 0.003),
            (0.7, false, 0.003),
        ]),
        ..BoreParams::at(0.4)
    });
    let bp = fd::render(&plain, 44_100, 0.1);
    let bc = fd::render(&closed, 44_100, 0.1);
    fd::sounds(&bp);

    let max_abs_p = bp.plane(0).iter().fold(0.0f64, |m, &s| m.max(s.abs()));
    let max_diff = bc
        .plane(0)
        .iter()
        .zip(bp.plane(0))
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f64, f64::max);
    let ratio = max_diff / max_abs_p;
    assert!(
        ratio > 1e-6,
        "closed holes should have SOME effect (eq. 118's C_c), not none: ratio {ratio}"
    );
    assert!(
        ratio < 0.10,
        "closed holes' effect should stay small, not swamp the plain bore: ratio {ratio}"
    );
}

/// A -60dB-off-peak floor excludes the noise bins near 0Hz `t_h`'s inertance leaves on top.
#[test]
fn opening_a_tonehole_raises_the_fundamental_relative_to_it_closed() {
    let fundamental_hz = |open: bool| -> f64 {
        let params = Params::DarabunditScavone(BoreParams {
            pulse_width: 0.0005,
            damp_dc: 0.0,
            damp_freq: 0.0,
            holes: holes(&[(0.5, open, 0.003)]),
            ..BoreParams::at(0.5)
        });
        let buffer = fd::render(&params, 11_025, 0.5);
        fd::sounds(&buffer);
        let s = sva_samples::measure::spectrum::analyze(buffer.plane(0), 11_025.0, 24, None);
        let loudest = s.peaks.iter().fold(f64::NEG_INFINITY, |m, p| m.max(p.db));
        s.peaks
            .iter()
            .filter(|p| p.db > loudest - 60.0)
            .map(|p| p.hz)
            .fold(f64::INFINITY, f64::min)
    };
    let (closed, open) = (fundamental_hz(false), fundamental_hz(true));
    assert!(
        open > closed * 1.1,
        "opening the hole should raise the fundamental: closed {closed}Hz open {open}Hz"
    );
}

/// Regression: two holes on one grid node under-counted that node's admittance.
#[test]
fn two_holes_rounding_to_the_same_grid_node_still_step_finite_and_stable() {
    let params = Params::DarabunditScavone(BoreParams {
        holes: holes(&[(0.4, true, 0.002), (0.6, false, 0.003)]),
        ..BoreParams::at(0.18)
    });
    fd::sounds(&fd::render(&params, 8_000, 0.1));
}

#[test]
fn a_parameter_out_of_range_is_refused() {
    let base = || BoreParams::at(0.4);
    fd::refuses(
        &Params::DarabunditScavone(base()),
        [
            BoreParams {
                length: 0.0,
                ..base()
            },
            BoreParams {
                radius_in: 0.0,
                ..base()
            },
            BoreParams {
                radius_out: -1.0,
                ..base()
            },
            BoreParams {
                excite_pos: 1.0,
                ..base()
            },
            BoreParams {
                pulse_width: 0.0,
                ..base()
            },
            BoreParams {
                damp_dc: -0.1,
                ..base()
            },
            BoreParams {
                length: f64::NAN,
                ..base()
            },
        ]
        .into_iter()
        .map(Params::DarabunditScavone)
        .collect(),
    );
}

#[test]
fn an_undamped_run_stays_bounded() {
    fd::stays_bounded(
        &Params::DarabunditScavone(BoreParams {
            damp_dc: 0.0,
            damp_freq: 0.0,
            ..BoreParams::at(0.4)
        }),
        44_100,
        0.3,
    );
}
