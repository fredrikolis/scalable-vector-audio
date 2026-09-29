// Concern: states what per-band crest owes a tone, a click track and silence | Non-concern: the ERB bank (src/measure/bands.rs) | IO: (samples) -> asserted dB

use sva_samples::measure::crest::{BandCrest, Crest, analyze};

fn at(hz: f64, sr: f64, secs: f64, amp: f64) -> Vec<f64> {
    (0..(secs * sr) as usize)
        .map(|i| amp * (2.0 * std::f64::consts::PI * hz * i as f64 / sr).sin())
        .collect()
}

fn band_at(c: &Crest, hz: f64) -> BandCrest {
    *c.bands
        .iter()
        .find(|b| b.lo_hz <= hz && hz < b.hi_hz)
        .expect("a third-octave band holds it")
}

#[test]
fn a_steady_sine_reads_three_decibels_of_crest_in_its_own_band() {
    let c = analyze(&at(1000.0, 44100.0, 2.0, 0.5), 44100.0);
    let own = band_at(&c, 1000.0);
    assert!(own.counted);
    assert!(
        (own.crest_db - 3.01).abs() < 0.2,
        "{} dB in {} Hz",
        own.crest_db,
        own.centre_hz
    );
    assert!((c.broadband_crest_db - 3.01).abs() < 0.1);
    assert!(
        c.spread_db.unwrap() < 1.5,
        "one tone is one dynamic, whatever the level per band: {:?}",
        c.spread_db
    );
}

#[test]
fn a_band_of_clicks_beside_a_band_of_drone_reads_as_spread() {
    let sr = 44100.0;
    let mut mixed = at(100.0, sr, 2.0, 0.3);
    for (i, x) in mixed.iter_mut().enumerate() {
        if i % 8820 == 0 {
            *x += 0.6;
        }
    }
    let c = analyze(&mixed, sr);
    let drone = band_at(&c, 100.0);
    let clicky = band_at(&c, 6000.0);
    assert!(clicky.crest_db > drone.crest_db + 6.0, "{c:?}");
    assert!(c.spread_db.unwrap() > 6.0);
    assert!(c.widest_band_hz.unwrap() > c.tightest_band_hz.unwrap());
}

#[test]
fn a_band_far_under_the_loudest_is_not_counted_toward_the_spread() {
    let c = analyze(&at(1000.0, 44100.0, 2.0, 0.5), 44100.0);
    assert!(!band_at(&c, 40.0).counted, "under the tone by five octaves");
    assert!(!band_at(&c, 15000.0).counted, "over it by four");
    assert!(c.bands.iter().filter(|b| b.counted).count() < c.bands.len());
    assert_eq!(c.counted_under_db, 60.0);
}

#[test]
fn silence_carries_no_spread_and_no_counted_band() {
    let c = analyze(&vec![0.0; 4410], 44100.0);
    assert_eq!(c.spread_db, None);
    assert!(c.bands.iter().all(|b| !b.counted));
}
