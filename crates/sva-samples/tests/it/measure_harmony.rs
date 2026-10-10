// Concern: what harmonic readings owe textbook chords: roughness minima, voice leadings, Tonnetz paths, measured partials | Non-concern: closed forms | IO: (partials or samples) -> asserts

use sva_samples::measure::harmony::chords::{Chord, segment};
use sva_samples::measure::harmony::dissonance::{Crowded, Dissonance, measure, pair};
use sva_samples::measure::harmony::tonnetz::{Quality, Step, Triad, analyze, path};
use sva_samples::measure::harmony::voice_leading::{Norm, lead};
use sva_samples::measure::harmony::{Framing, Moment, Partial, measured};

const C4: f64 = 261.625_565_300_598_6;
const PAIRS: usize = 1 << 17;

fn exact(hz: f64, amplitude: f64) -> Partial {
    Partial {
        hz,
        amplitude,
        hz_bounds: (hz, hz),
        amplitude_bounds: (amplitude, amplitude),
    }
}

fn rough(moments: &[Moment], framing: Framing, floor_db: f64) -> Dissonance {
    measure(moments, framing, floor_db, PAIRS).expect("few partials")
}

fn hz(midi: i64) -> f64 {
    440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0)
}

/// Six harmonics falling 0.88 per partial, the timbre of Sethares' Fig. 2.
fn harmonic_tone(f0: f64) -> Vec<Partial> {
    (1..=6)
        .map(|n| exact(f0 * f64::from(n), 0.88f64.powi(n - 1)))
        .collect()
}

fn chord(midis: &[i64]) -> Moment {
    Moment {
        t_secs: 0.0,
        partials: midis.iter().flat_map(|m| harmonic_tone(hz(*m))).collect(),
    }
}

const EXACT: Framing = Framing {
    hop_secs: 0.05,
    window: None,
};

fn roughness(moment: Moment) -> f64 {
    rough(&[moment], EXACT, -120.0).frames[0].roughness
}

/// Eq. (1)'s maximum sits at x = ln(b/a)/(b - a), which s scales to d* there.
#[test]
fn one_pair_peaks_where_sethares_scaled_it_and_vanishes_at_unison() {
    assert_eq!(pair(C4, C4, 1.0, 1.0), 0.0);
    let s = 0.24 / (0.021 * C4 + 19.0);
    let x_star = (5.75f64 / 3.5).ln() / (5.75 - 3.5);
    let top = pair(C4, C4 + x_star / s, 1.0, 1.0);
    for df in [1.0, 5.0, 40.0, 80.0] {
        assert!(pair(C4, C4 + df, 1.0, 1.0) < top, "{df} Hz is off the peak");
    }
    assert!((pair(C4, 2.0 * C4, 0.5, 0.25) - 0.125 * pair(C4, 2.0 * C4, 1.0, 1.0)).abs() < 1e-15);
}

/// With harmonic tones the fifth and the octave are local minima of Sethares' curve, and the
/// major and minor triads are far smoother than a cluster.
#[test]
fn consonant_intervals_and_triads_are_smoother_than_their_neighbours_and_clusters() {
    let interval = |semis: f64| {
        let mut two = harmonic_tone(C4);
        two.extend(harmonic_tone(C4 * 2f64.powf(semis / 12.0)));
        roughness(Moment {
            t_secs: 0.0,
            partials: two,
        })
    };
    for (just, at) in [(1.5f64, "fifth"), (2.0, "octave")] {
        let semis = 12.0 * just.log2();
        assert!(
            interval(semis) < interval(semis - 0.3),
            "{at} below its flat side"
        );
        assert!(
            interval(semis) < interval(semis + 0.3),
            "{at} below its sharp side"
        );
    }
    let major = roughness(chord(&[60, 64, 67]));
    let minor = roughness(chord(&[60, 63, 67]));
    let cluster = roughness(chord(&[60, 61, 62]));
    assert!(major < 0.5 * cluster, "{major} vs {cluster}");
    assert!(minor < 0.5 * cluster, "{minor} vs {cluster}");
}

#[test]
fn the_curve_reports_its_mean_and_where_it_peaks_and_rests() {
    let mut moments = vec![chord(&[60, 64, 67]), chord(&[60, 61, 62]), chord(&[60, 67])];
    for (k, m) in moments.iter_mut().enumerate() {
        m.t_secs = k as f64 * 0.05;
    }
    let read = rough(&moments, EXACT, -120.0);
    let values: Vec<f64> = read.frames.iter().map(|f| f.roughness).collect();
    let max = read.max.expect("frames");
    let min = read.min.expect("frames");
    assert_eq!((max.t_secs, max.value), (0.05, values[1]));
    assert_eq!((min.t_secs, min.value), (0.1, values[2]));
    assert!((read.mean.expect("frames") - values.iter().sum::<f64>() / 3.0).abs() < 1e-15);
}

/// Partials under the floor are left out, and those over it pair or the reading refuses.
#[test]
fn the_floor_drops_quiet_partials_and_too_many_pairs_refuse() {
    let quiet = Moment {
        t_secs: 0.0,
        partials: vec![exact(440.0, 1.0), exact(450.0, 0.001)],
    };
    let kept = |floor: f64| rough(std::slice::from_ref(&quiet), EXACT, floor).frames[0];
    assert_eq!(kept(-40.0).roughness, 0.0);
    assert!(kept(-80.0).roughness > 0.0);
    assert_eq!(
        kept(-80.0).roughness_bounds,
        (kept(-80.0).roughness, kept(-80.0).roughness)
    );
    let crowd = Moment {
        t_secs: 0.25,
        partials: (0..1000)
            .map(|k| exact(100.0 + f64::from(k), 1.0))
            .collect(),
    };
    assert_eq!(
        measure(std::slice::from_ref(&crowd), EXACT, -120.0, PAIRS),
        Err(Crowded {
            t_secs: 0.25,
            partials: 1000,
            pairs: 499_500,
            cap: PAIRS
        })
    );
    assert!(
        measure(&[crowd], EXACT, -120.0, 499_500).is_ok(),
        "the cap is the caller's"
    );
}

/// G3 B3 D4 to C4 E4 G4 voiced closest: B3 up to C4, D4 to E4, and G3 rises to G4 or doubles.
#[test]
fn dominant_to_tonic_moves_by_the_textbook_smallest_voice_leading() {
    let (d, mapping) = lead(&[55, 59, 62], &[55, 60, 64], Norm::L1);
    assert_eq!(d, 3.0);
    assert_eq!(mapping, vec![(55, 55), (59, 60), (62, 64)]);
    let (d2, _) = lead(&[55, 59, 62], &[55, 60, 64], Norm::L2);
    assert!((d2 - 5f64.sqrt()).abs() < 1e-15);
    let (seventh, mapping) = lead(&[55, 59, 62, 65], &[60, 64], Norm::L1);
    assert_eq!(seventh, 5.0 + 1.0 + 2.0 + 1.0, "{mapping:?}");
}

#[test]
fn a_held_chord_leads_nowhere_and_a_doubled_note_is_a_move_of_its_own() {
    assert_eq!(lead(&[60, 64, 67], &[60, 64, 67], Norm::L1).0, 0.0);
    let (d, mapping) = lead(&[60], &[60, 64, 67], Norm::L1);
    assert_eq!((d, mapping), (11.0, vec![(60, 60), (60, 64), (60, 67)]));
}

fn triad(root: u8, quality: Quality) -> Triad {
    Triad { root, quality }
}

const MAJOR: Quality = Quality::Major;
const MINOR: Quality = Quality::Minor;

#[test]
fn triads_are_named_by_their_pitch_classes_whatever_the_voicing() {
    assert_eq!(Triad::of(&[48, 64, 67, 72]), Some(triad(0, MAJOR)));
    assert_eq!(Triad::of(&[57, 60, 64]), Some(triad(9, MINOR)));
    assert_eq!(Triad::of(&[60, 64, 67, 71]), None, "a seventh chord");
    assert_eq!(Triad::of(&[60, 61, 62]), None);
}

/// ii-V-I in C: d to G is three steps (d P D R b L G), G to C two (G R e L C).
/// I-V-vi-IV: C to G two, G to a three, a to F one (a L F).
#[test]
fn the_tonnetz_counts_the_textbook_neo_riemannian_paths() {
    let (c, d, f, g, a) = (
        triad(0, MAJOR),
        triad(2, MINOR),
        triad(5, MAJOR),
        triad(7, MAJOR),
        triad(9, MINOR),
    );
    assert_eq!(Step::R.of(c), a);
    assert_eq!(Step::L.of(c), triad(4, MINOR));
    assert_eq!(Step::P.of(c), triad(0, MINOR));
    let walked = |from: Triad, steps: Vec<Step>| steps.into_iter().fold(from, |t, s| s.of(t));
    assert_eq!(path(d, g), vec![Step::P, Step::R, Step::L]);
    assert_eq!(walked(d, path(d, g)), g);
    assert_eq!(walked(g, path(g, a)), a);
    assert_eq!(path(g, c).len(), 2);
    assert_eq!(path(c, g).len(), 2);
    assert_eq!(path(g, a).len(), 3);
    assert_eq!(path(a, f), vec![Step::L]);
    assert!(path(c, c).is_empty());
}

fn at(start: f64, notes: &[i64]) -> Chord {
    Chord {
        start_secs: start,
        end_secs: start + 1.0,
        notes: notes.to_vec(),
        unresolved: Vec::new(),
    }
}

#[test]
fn tension_is_each_chords_distance_from_the_first_triad() {
    let read = analyze(
        vec![
            at(0.0, &[60, 61]),
            at(1.0, &[62, 65, 69]),
            at(2.0, &[55, 59, 62]),
            at(3.0, &[48, 64, 67]),
        ],
        EXACT,
        -40.0,
    );
    assert_eq!(read.tonic, Some((triad(2, MINOR), 1.0)));
    let tension: Vec<Option<usize>> = read.chords.iter().map(|c| c.tension).collect();
    assert_eq!(tension, vec![None, Some(0), Some(3), Some(3)]);
    assert_eq!(read.transitions[0].path, None, "a dyad is no triad");
    assert_eq!(read.transitions[1].path.as_ref().map(Vec::len), Some(3));
}

fn render(partials: &[(f64, f64)], rate: f64, secs: f64) -> Vec<f64> {
    (0..(rate * secs) as usize)
        .map(|n| {
            let t = n as f64 / rate;
            partials
                .iter()
                .map(|(f, a)| a * (std::f64::consts::TAU * f * t).cos())
                .sum()
        })
        .collect()
}

/// A C major triad of pure tones measured off samples: each frame finds the three partials
/// and no leakage peak, so its chord matches and its roughness is within 1% of the exact
/// partials' (0.2% as measured here), inside the bounds each frame reports.
#[test]
fn measured_partials_find_a_triads_tones_and_its_exact_roughness() {
    let tones: Vec<(f64, f64)> = [60, 64, 67].iter().map(|m| (hz(*m), 0.3)).collect();
    let samples = render(&tones, 44_100.0, 0.5);
    let (moments, framing) = measured::moments(&samples, 44_100.0, 0.0, 0.05);
    let window = framing.window.expect("measured frames name their window");
    assert_eq!((window.size, moments.len()), (4096, 10));
    let exact = Moment {
        t_secs: 0.0,
        partials: tones.iter().map(|(hz, a)| exact(*hz, *a)).collect(),
    };
    let truth = roughness(exact);
    let steady = &moments[2..8];
    for m in steady {
        assert_eq!(m.partials.len(), 3, "{m:?}");
        let frame = rough(std::slice::from_ref(m), framing, -40.0).frames[0];
        let (found, (lo, hi)) = (frame.roughness, frame.roughness_bounds);
        assert!((found - truth).abs() <= 0.01 * truth, "{found} vs {truth}");
        assert!(lo <= truth && truth <= hi, "{truth} in [{lo}, {hi}]");
        for p in &m.partials {
            assert!(p.hz_bounds.0 <= p.hz && p.hz <= p.hz_bounds.1, "{p:?}");
        }
    }
    let chords = segment(steady, framing.hop_secs, -40.0);
    assert_eq!(chords.len(), 1);
    assert_eq!(chords[0].notes, vec![60, 64, 67]);
    assert!(chords[0].unresolved.is_empty(), "{:?}", chords[0]);
}

/// A lone sinusoid is what Grandke's interpolation is exact for, so its bounds are a point.
#[test]
fn a_lone_tones_measured_bounds_are_a_point() {
    let samples = render(&[(440.0, 0.5)], 44_100.0, 0.3);
    let (moments, _) = measured::moments(&samples, 44_100.0, 0.0, 0.05);
    let [p] = moments[3].partials[..] else {
        panic!("one partial: {:?}", moments[3])
    };
    assert_eq!(
        (p.hz_bounds, p.amplitude_bounds),
        ((p.hz, p.hz), (p.amplitude, p.amplitude))
    );
    assert!(
        (p.hz - 440.0).abs() < 0.05 && (p.amplitude - 0.5).abs() < 1e-3,
        "{p:?}"
    );
}
