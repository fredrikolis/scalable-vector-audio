// Concern: states what the stereo image owes correlated, inverted and panned pairs | Non-concern: level per component (src/measure/envelope.rs) | IO: (two planes) -> asserted image

use sva_samples::measure::stereo::{RAIL_DB, StereoFrame, analyze};

fn image(l: Vec<f64>, r: Vec<f64>) -> StereoFrame {
    analyze(&[&l, &r], 2, 100.0, 0.0, 1.0).overall
}

/// A keyed hash, so a run is deterministic without depending on an evaluator.
fn splitmix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn drawn(seed: f64, index: i64) -> f64 {
    let bits = splitmix64((index as u64) ^ splitmix64(seed.to_bits()));
    (bits >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

fn noise(seed: f64, n: usize) -> Vec<f64> {
    (0..n).map(|i| drawn(seed, i as i64)).collect()
}

#[test]
fn identical_components_read_as_centred_mono_that_survives_fold_down() {
    let n = noise(1.0, 4000);
    let f = image(n.clone(), n);
    assert!((f.correlation - 1.0).abs() < 1e-9);
    assert!(f.side_rms < 1e-9, "nothing in the sides: {}", f.side_rms);
    assert!(f.width < 1e-9);
    assert!(f.balance_db.abs() < 1e-9);
    assert!(f.mono_db.abs() < 1e-9, "nothing cancels: {}", f.mono_db);
}

#[test]
fn an_inverted_component_cancels_completely_on_fold_down() {
    let n = noise(2.0, 4000);
    let f = image(n.clone(), n.iter().map(|x| -x).collect());
    assert!((f.correlation + 1.0).abs() < 1e-9);
    assert!(f.mid_rms < 1e-9);
    assert!(
        f.mono_db < -100.0,
        "the whole signal cancels: {}",
        f.mono_db
    );
}

#[test]
fn decorrelated_components_sit_near_zero_and_lose_3_db_to_fold_down() {
    let f = image(noise(3.0, 20000), noise(4.0, 20000));
    assert!(f.correlation.abs() < 0.05, "correlation {}", f.correlation);
    assert!(
        (f.width - 1.0).abs() < 0.05,
        "S and M are equal: {}",
        f.width
    );
    assert!(
        (f.mono_db + 3.0).abs() < 0.3,
        "summing two independent signals halves the power: {}",
        f.mono_db
    );
}

/// A gain difference is balance, not decorrelation — the whole point of using Pearson.
#[test]
fn a_level_difference_moves_balance_and_leaves_correlation_alone() {
    let n = noise(5.0, 4000);
    let f = image(n.clone(), n.iter().map(|x| x * 0.5).collect());
    assert!((f.correlation - 1.0).abs() < 1e-9);
    assert!((f.balance_db + 6.0206).abs() < 0.01, "{}", f.balance_db);
    assert!(
        f.width > 0.3,
        "half on one side is audibly wide: {}",
        f.width
    );
}

#[test]
fn a_silent_side_reads_as_hard_panned_rather_than_centred() {
    let n = noise(6.0, 400);
    let hard = image(n.clone(), vec![0.0; 400]);
    assert_eq!(hard.balance_db, -RAIL_DB, "everything on the left");
    assert_eq!(image(vec![0.0; 400], n).balance_db, RAIL_DB);
    assert_eq!(image(vec![0.0; 400], vec![0.0; 400]).balance_db, 0.0);
}

/// A single number hides movement; frames are what make an auto-pan visible.
#[test]
fn per_frame_balance_follows_a_pan_a_whole_render_number_would_average_away() {
    let n = 400;
    let l: Vec<f64> = (0..n).map(|i| if i < n / 2 { 1.0 } else { 0.01 }).collect();
    let r: Vec<f64> = (0..n).map(|i| if i < n / 2 { 0.01 } else { 1.0 }).collect();
    let img = analyze(&[&l, &r], 2, 100.0, 0.0, 1.0);
    assert!(
        img.overall.balance_db.abs() < 0.01,
        "the average is centred"
    );
    assert_eq!(img.frames.len(), 4);
    assert!(img.frames[0].balance_db < -30.0, "first half hard left");
    assert!(img.frames[3].balance_db > 30.0, "second half hard right");
}
