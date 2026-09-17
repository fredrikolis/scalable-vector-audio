// Concern: proves every analysis that reports dB reads silence the same way | Non-concern: what any of them measures otherwise (the per-analysis suites) | IO: (silence) -> dB and the verdicts over it

use sva_analysis::{
    experimental::gain_reduction, experimental::masking, stable::trajectory, to_db,
};
use sva_samples::EnvelopeFrame;

const RATE: f64 = 44_100.0;
const FRAME: f64 = 0.05;

fn silent_frames() -> Vec<EnvelopeFrame> {
    (0..8)
        .map(|n| EnvelopeFrame {
            t_secs: n as f64 * FRAME,
            rms: 0.0,
            peak: 0.0,
        })
        .collect()
}

/// One conversion means one answer for silence, whichever analysis was asked.
#[test]
fn silence_reads_the_same_in_every_analysis_that_reports_db() {
    let silent = to_db(0.0);
    assert_eq!(silent, f64::NEG_INFINITY, "no floor, an exact absence");
    assert_eq!(to_db(1.0), 0.0, "full scale is 0 dB");
    assert!((to_db(0.5) + 6.0206).abs() < 1e-3, "{}", to_db(0.5));

    let envelope = silent_frames();
    let samples = vec![0.0f32; (RATE * FRAME * 8.0) as usize];
    let moved = trajectory::analyze(&envelope, None, &samples, RATE, FRAME);
    let gained = gain_reduction::analyze(&envelope, Some(&envelope));

    assert_eq!(moved.frames[0].rms_db, silent);
    assert_eq!(moved.frames[0].peak_db, silent);
    assert_eq!(gained.frames[0].gain_db, silent);
    assert_eq!(gained.frames[0].input_db, Some(silent));

    let masked = masking::analyze(&samples, &samples, RATE, 0.0, None).expect("masking answers");
    let band = &masked.bands[0];
    assert_eq!(band.target_db, silent);
    assert_eq!(band.against_db, silent);
    assert!(
        band.smr_db.is_nan(),
        "two silences leave no ratio: {}",
        band.smr_db
    );
}

fn leveled(rms: &[f64]) -> Vec<EnvelopeFrame> {
    silent_frames()
        .iter()
        .zip(rms)
        .map(|(f, &r)| EnvelopeFrame {
            rms: r,
            peak: r,
            ..*f
        })
        .collect()
}

fn direction(rms: &[f64]) -> Option<trajectory::Direction> {
    let samples = vec![0.0f32; (RATE * FRAME * 8.0) as usize];
    trajectory::analyze(&leveled(rms), None, &samples, RATE, FRAME)
        .level
        .map(|v| v.direction)
}

/// Every frame is `-inf` dB, so head minus tail is `NaN`, every comparison against it false, and
/// the fall-through once read "falling" — a decay nothing was there to make. A note that decays
/// *into* silence is the opposite case: `-inf` below a finite head is an unambiguous fall.
#[test]
fn a_silent_window_has_no_trajectory_verdict() {
    let samples = vec![0.0f32; (RATE * FRAME * 8.0) as usize];
    let silent = trajectory::analyze(&silent_frames(), None, &samples, RATE, FRAME);
    assert!(
        silent.frames.iter().all(|f| f.rms_db == to_db(0.0)),
        "the frames still report what they measured"
    );
    assert_eq!(silent.level, None, "and the window claims no direction");

    assert_eq!(
        direction(&[1.0, 0.5, 0.25, 0.125, 0.0625, 0.03125, 0.015625, 0.0078125]),
        Some(trajectory::Direction::Falling),
        "a window that sounds throughout names one"
    );
    assert_eq!(
        direction(&[1.0, 0.5, 0.25, 0.125, 0.0, 0.0, 0.0, 0.0]),
        Some(trajectory::Direction::Falling),
        "and so does one that decays into digital silence"
    );
    assert_eq!(
        direction(&[0.0, 0.0, 0.0, 0.0, 0.125, 0.25, 0.5, 1.0]),
        Some(trajectory::Direction::Rising),
        "and one that swells out of it"
    );
}

fn tone(hz: f64, secs: f64, amp: f32) -> Vec<f32> {
    (0..(secs * RATE) as usize)
        .map(|i| amp * (std::f64::consts::TAU * hz * i as f64 / RATE).sin() as f32)
        .collect()
}

/// `smr_db` cannot carry a `±inf`, so the two levels beside it are what a caller reads the
/// state off: which of them is `null` says whether the band was unmasked or fully masked.
#[test]
fn a_one_sided_silence_is_still_told_apart_by_the_levels_beside_the_ratio() {
    let loud = tone(1000.0, 0.5, 0.5);
    let quiet = vec![0.0f32; loud.len()];

    let unmasked = masking::analyze(&loud, &loud, RATE, 0.0, None).expect("masking answers");
    let band = unmasked
        .bands
        .iter()
        .find(|b| b.lo_hz <= 1000.0 && 1000.0 < b.hi_hz)
        .expect("the band the tone sits in");
    assert!(band.target_db.is_finite(), "the target sounded");
    assert_eq!(band.against_db, to_db(0.0), "nothing was left to mask it");
    assert!(!band.smr_db.is_finite(), "{}", band.smr_db);

    let buried = masking::analyze(&quiet, &loud, RATE, 0.0, None).expect("masking answers");
    let band = buried
        .bands
        .iter()
        .find(|b| b.lo_hz <= 1000.0 && 1000.0 < b.hi_hz)
        .expect("the band the masker sits in");
    assert_eq!(band.target_db, to_db(0.0), "the target was silent");
    assert!(band.against_db.is_finite(), "and something masked it");
    assert!(!band.smr_db.is_finite(), "{}", band.smr_db);
}
