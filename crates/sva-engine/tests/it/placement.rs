// Concern: proves a placed read shares its node's one value: computed once, zero-skipped, folded, snapped | Non-concern: what a value holds | IO: (a composition) -> evaluated segments, onsets

use crate::fixtures::{Now, graph_of, next};
use sva_ast::{Graph, PerBar};
use sva_engine::{
    Extent, Outcome, Range, RenderConfig, Stream, StreamConfig, Tier, identity, render,
};

const RATE: u32 = 44_100;

/// The samples `evaluated` covers where it computed each one once, as one span.
fn once(evaluated: &[Extent]) -> Option<Extent> {
    let mut held = evaluated.to_vec();
    held.sort_by_key(|e| e.start);
    let mut end = held.first()?.start;
    for e in &held {
        if e.start != end {
            return None;
        }
        end = e.end;
    }
    Some(Extent::new(held[0].start, end))
}

fn at_tempo(name: &str, files: &[(&str, &str)], seconds_per_bar: f64) -> Graph {
    let mut g = graph_of(name, files);
    g.resolve_bar_spans(PerBar {
        seconds: seconds_per_bar,
        per: 1.0,
    });
    g
}

/// At 128 bpm 4.1 bars is no whole count of samples, and still one value; the label states
/// how far the render moved a read to land it, here in samples.
#[test]
fn a_note_read_at_three_placements_is_computed_once_at_either_tempo() {
    let note = "crop(sin(2*pi*440*t)*exp(0 - t/0.2), 0s, 1b)\n";
    for bpm in [120.0, 128.0] {
        let bar = 4.0 * 60.0 / bpm;
        let len = (bar * f64::from(RATE)).ceil() as i64;
        let fast = bpm == 128.0;
        for (song, reads, moved) in [
            (
                "@n(t) + @n(t - 4b) + @n(t - 4.1b)\n",
                3,
                if fast { 0.25 } else { 0.0 },
            ),
            (
                "@n(t - 3ms) + @n(t - 3b)\n",
                2,
                if fast { 0.5 } else { 0.3 },
            ),
        ] {
            let g = at_tempo("placement", &[("n", note), ("song", song)], bar);
            let held =
                render(&g, "song", RenderConfig::at(RATE), &Tier::default()).expect("a song");
            let n = held.id("n").expect("the note");
            let computed = held.evaluated(n);
            assert_eq!(
                once(&computed),
                Some(Extent::new(0, len)),
                "{bpm}: {song}: {computed:?}"
            );
            let said = held.labels[&held.root].moved.expect("samples computed");
            assert!(
                (said * f64::from(RATE) - moved).abs() < 1e-9,
                "{bpm}: {song}: {said}"
            );
            let stats = held.cache_stats.expect("what the render asked");
            let asked: Vec<Outcome> = stats
                .lookups
                .iter()
                .filter(|l| l.node == "n")
                .map(|l| l.outcome)
                .collect();
            assert_eq!(asked.len(), reads, "{bpm}: one lookup per read: {asked:?}");
            assert!(
                asked[1..].iter().all(|o| *o == Outcome::Reused),
                "{asked:?}"
            );
        }
    }
}

#[test]
fn a_crop_skips_what_it_reads_outside_it_before_it_is_computed() {
    let g = graph_of(
        "crop-skips",
        &[
            ("long", "exp(0 - t/60)*sin(2*pi*220*t)\n"),
            ("cut", "crop(@long(t), 0s, 1s)\n"),
        ],
    );
    let held = render(&g, "cut", RenderConfig::at(RATE), &Tier::default()).expect("a crop");
    let long = held.id("long").expect("the decay");
    let computed = held.evaluated(long);
    assert_eq!(
        once(&computed),
        Some(Extent::new(0, i64::from(RATE))),
        "{computed:?}"
    );
}

fn streamed(
    g: &Graph,
    target: &str,
    rate: u32,
    seconds: i64,
    each: &mut dyn FnMut(i64, &[f64]),
) -> Stream {
    let config = StreamConfig {
        block: 1 << 15,
        channels: None,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(seconds * i64::from(rate)),
            },
            ..RenderConfig::at(rate)
        },
    };
    let at = sva_ast::parse_expr(target).expect("a target");
    let mut stream = Stream::open(g, &at, config, &Tier::default())
        .now()
        .expect("it streams");
    while let Some(block) = next(&mut stream).expect("a block") {
        each(block.start, block.plane(0));
    }
    stream
}

/// 0.25 Hz repeats every 176,400 samples at 44.1 kHz.
#[test]
fn a_periodic_value_over_three_minutes_computes_one_period() {
    let g = graph_of("periodic", &[("lfo", "sin(2*pi*0.25*t)\n")]);
    let stream = streamed(&g, "@lfo", RATE, 180, &mut |_, _| {});
    let computed = stream.evaluated("lfo");
    assert_eq!(
        once(&computed),
        Some(Extent::new(0, 176_400)),
        "{computed:?}"
    );
}

/// A noise whose lines repeat only after 882,000,000 samples, read a sample early: the read
/// folds to both ends of the period, and lays only the samples it reads, never the period.
#[test]
fn a_read_folded_to_both_ends_of_a_long_period_lays_only_what_it_reads() {
    let g = graph_of(
        "long-period",
        &[
            (
                "nz",
                "bandpass(noise(9, period=8/184.9972, color=-3), 6500, 0.5)\n",
            ),
            ("top", "crop(sample(@nz(t - 1sp)), 0s, 0.05s)\n"),
        ],
    );
    let (rendered, bytes) = crate::allocations::largest(|| {
        render(&g, "top", RenderConfig::at(RATE), &Tier::default())
            .unwrap_or_else(|e| panic!("{e}"))
    });
    let nz = rendered.id("nz").expect("the noise");
    let computed = rendered.evaluated(nz);
    let folded = [Extent::new(0, 2_204), Extent::new(881_999_999, 882_000_000)];
    assert!(
        computed
            .iter()
            .all(|e| folded.iter().any(|f| e.intersect(*f) == *e)),
        "{computed:?}"
    );
    assert!(bytes < 1 << 26, "one allocation asked {bytes} bytes");
}

/// 384 beats at 128 bpm, 22,050 Hz: the last as close as the first.
#[test]
fn every_onset_lands_within_half_a_sample_and_the_tempo_never_drifts() {
    let rate = 22_050;
    let beat = 60.0 / 128.0;
    let bars: Vec<(String, String)> = (0..24)
        .map(|bar| {
            let clicks: Vec<String> = (0..16)
                .map(|k| format!("@click(t - {}b)", 16 * bar + k))
                .collect();
            (format!("bar{bar}"), format!("{}\n", clicks.join(" + ")))
        })
        .collect();
    let song: Vec<String> = bars.iter().map(|(name, _)| format!("@{name}")).collect();
    let song = format!("{}\n", song.join(" + "));
    let mut files: Vec<(&str, &str)> = vec![("click", "crop(1, 0s, 1sp)\n"), ("song", &song)];
    files.extend(
        bars.iter()
            .map(|(name, body)| (name.as_str(), body.as_str())),
    );
    let g = at_tempo("snapped", &files, beat);
    let mut onsets = Vec::new();
    streamed(&g, "@song", rate, 180, &mut |start, plane| {
        onsets.extend(
            plane
                .iter()
                .enumerate()
                .filter(|(_, v)| **v != 0.0)
                .map(|(i, v)| (start + i as i64, *v)),
        );
    });
    assert_eq!(onsets.len(), 384);
    for (k, (at, v)) in onsets.iter().enumerate() {
        let exact = k as f64 * beat * f64::from(rate);
        assert_eq!(*v, 1.0, "beat {k}");
        assert!(
            (*at as f64 - exact).abs() <= 0.5,
            "beat {k}: {at} for {exact}"
        );
        let even = exact.round_ties_even() as i64;
        assert_eq!(*at, even, "beat {k}: ties round to even");
    }
}

#[test]
fn no_identity_or_body_holds_a_reads_shift() {
    let files: [(&str, &str); 3] = [
        ("n", "crop(sin(2*pi*440*t), 0s, 0.5s)\n"),
        ("near", "@n(t - 3ms)\n"),
        ("far", "@n(t - 3b)\n"),
    ];
    let g = at_tempo("placed", &files, 2.0);
    let of = |root: &str| {
        let typing = sva_engine::types(&g, root).expect("a law");
        let n = typing.id("n").expect("the note");
        let sva_engine::Value::ClosedForm(form) = typing.value(n) else {
            panic!("a law");
        };
        assert!(
            !format!("{:?}", form.body).contains("Shift"),
            "{root}: n's body holds no shift"
        );
        identity(&typing, n).expect("an identity")
    };
    assert_eq!(of("near"), of("far"), "one identity wherever it is read");
}
