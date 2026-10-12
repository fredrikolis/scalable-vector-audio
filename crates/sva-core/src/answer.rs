// Concern: writes one reading and the render around it as the `data` object | Non-concern: the error envelope (output.rs), subcommand JSON (sva-cli) | IO: (Answer) -> a JSON string

use std::path::Path;

use sva_engine::{
    Alias, AliasBand, Answer, Arguments, BandCrest, BandTrack, Bands, Binding, Buffer, CacheStats,
    Counters, Crest, CuttingBelowSilenceThreshold, Detail, EngineStats, EnvelopeFrame,
    FormantFrame, Kind, Label, LedgerEntry, Loudness, LoudnessFrame, Onsets, Outcome, Output,
    PayloadKind, Phase, Source, SpectralSum, Spectrum, StereoFrame, StereoImage, Work, Worked,
};

use sva_engine::harmony::Framing;
use sva_engine::harmony::chords::{self, Chord};
use sva_engine::harmony::dissonance::{self, Dissonance, Extreme};
use sva_engine::harmony::inharmonicity::{self, Inharmonicity, Series, Weighted};
use sva_engine::harmony::tonnetz::{self, Quality, TonalDistance, Triad};
use sva_engine::harmony::voice_leading::{self, VoiceLeading};
use sva_engine::modulation::{self, Modulation};
use sva_engine::periodicity::{self, Periodicity, Track, Window};
use sva_engine::pitch as pitch_names;
use sva_engine::self_similarity::{self, SelfSimilarity};
use sva_engine::sliding::Stats;

use crate::json::{NONE, capped, escape, latest, list, num};

/// Past this a caller reading stdout wants a later interval start, not a wall of JSON.
pub const SAMPLE_LIMIT: usize = 4096;

fn maybe(v: Option<f64>) -> String {
    v.map(num).unwrap_or_else(|| NONE.to_string())
}

fn counted(v: Option<usize>) -> String {
    v.map_or_else(|| NONE.to_string(), |c| c.to_string())
}

fn ledger_json(entries: &[LedgerEntry], skim: bool) -> String {
    list(entries, |e| {
        let clipped = e
            .clipped
            .map_or_else(|| NONE.to_string(), |c| c.to_string());
        let channel = counted(e.channel);
        if skim {
            return format!(
                "\n    {{ \"node\": \"{}\", \"channel\": {channel}, \"rms\": {}, \"peak\": {}, \"clipped\": {clipped} }}",
                escape(&e.node),
                num(e.rms),
                num(e.peak)
            );
        }
        format!(
            "\n    {{ \"node\": \"{}\", \"channel\": {channel}, \"depth\": {}, \"unit\": \"{}\", \
             \"control\": {}, \"rms\": {}, \"peak\": {}, \"min\": {}, \"max\": {}, \
             \"share\": {}, \"clipped\": {clipped} }}",
            escape(&e.node),
            e.depth,
            e.kind.unit().name(),
            !e.kind.is_audio(),
            num(e.rms),
            num(e.peak),
            num(e.min),
            num(e.max),
            maybe(e.share)
        )
    })
}

fn spectrum_json(s: &Spectrum) -> String {
    format!(
        "{{ \"frame_size\": {}, \"frames\": {}, \"resolution_hz\": {}, \"rms\": {}, \
         \"centroid_hz\": {}, \"rolloff85_hz\": {}, \"peaks\": {}, \"bands\": {} }}",
        s.frame_size,
        s.frames,
        num(s.resolution_hz),
        num(s.rms),
        num(s.centroid_hz),
        num(s.rolloff85_hz),
        list(&s.peaks, |p| format!(
            "{{ \"hz\": {}, \"db\": {} }}",
            num(p.hz),
            num(p.db)
        )),
        list(&s.bands, |b| format!(
            "\n    {{ \"lo_hz\": {}, \"hi_hz\": {}, \"db\": {} }}",
            num(b.lo_hz),
            num(b.hi_hz),
            num(b.db)
        ))
    )
}

fn stereo_json(image: &StereoImage) -> String {
    let frame = |f: &StereoFrame| {
        format!(
            "\n      {{ \"t\": {}, \"correlation\": {}, \"mid_rms\": {}, \"side_rms\": {}, \
             \"width\": {}, \"balance_db\": {}, \"mono_db\": {} }}",
            num(f.t_secs),
            num(f.correlation),
            num(f.mid_rms),
            num(f.side_rms),
            num(f.width),
            num(f.balance_db),
            num(f.mono_db)
        )
    };
    format!(
        "{{ \"channels\": {}, \"overall\": {}, \"frames\": {} }}",
        image.channels,
        frame(&image.overall),
        list(&image.frames, frame)
    )
}

fn formants_json(f: &FormantFrame) -> String {
    format!(
        "\n    {{ \"t\": {}, \"order\": {}, \"energy\": {}, \"residual\": {}, \"formants\": {} }}",
        num(f.t_secs),
        f.order,
        num(f.energy),
        num(f.residual),
        list(&f.formants, |v| format!(
            "{{ \"hz\": {}, \"bandwidth_hz\": {}, \"db\": {} }}",
            num(v.hz),
            num(v.bandwidth_hz),
            num(v.db)
        ))
    )
}

fn band_json(b: &BandTrack, start_secs: f64, rate_hz: f64, limit: Option<usize>) -> String {
    let shown = limit.map_or(b.rms.len(), |cap| b.rms.len().min(cap));
    format!(
        "\n    {{ \"centre_hz\": {}, \"cam\": {}, \"erb_hz\": {}, \"q\": {}, \"peak\": {}, \
         \"time_to_peak_secs\": {}, \"rise_10_90_secs\": {}, \
         \"floor\": {{ \"peak\": {}, \"time_to_peak_secs\": {}, \"rise_10_90_secs\": {} }}, \
         \"rms\": {} }}",
        num(b.centre_hz),
        num(b.cam),
        num(b.erb_hz),
        num(b.q),
        num(b.peak),
        maybe(b.time_to_peak_secs),
        maybe(b.rise_10_90_secs),
        num(b.floor.peak),
        num(b.floor.time_to_peak_secs),
        maybe(b.floor.rise_10_90_secs),
        capped(
            &b.rms,
            shown,
            |n| start_secs + n as f64 / rate_hz,
            |v| num(*v),
        )
    )
}

fn bands_json(b: &Bands, limit: Option<usize>) -> String {
    format!(
        "{{ \"rate_hz\": {}, \"start_secs\": {}, \"bands\": {} }}",
        num(b.rate_hz),
        num(b.start_secs),
        list(&b.bands, |band| band_json(
            band,
            b.start_secs,
            b.rate_hz,
            limit
        ))
    )
}

fn loudness_json(l: &Loudness, limit: Option<usize>) -> String {
    let frames = |fs: &[LoudnessFrame]| {
        let shown = limit.map_or(fs.len(), |cap| fs.len().min(cap));
        capped(
            fs,
            shown,
            |n| fs[n].t,
            |f| format!("\n    {{ \"t\": {}, \"lufs\": {} }}", num(f.t), num(f.lufs)),
        )
    };
    format!(
        "{{ \"integrated_lufs\": {}, \"range_lu\": {}, \"momentary_max_lufs\": {}, \
         \"short_term_max_lufs\": {}, \"sample_peak\": {}, \"sample_peak_dbfs\": {}, \
         \"peak_note\": \"{}\", \"momentary\": {}, \"short_term\": {} }}",
        maybe(l.integrated_lufs),
        maybe(l.range_lu),
        maybe(l.momentary_max_lufs),
        maybe(l.short_term_max_lufs),
        num(l.sample_peak),
        maybe(l.sample_peak_dbfs),
        escape(l.peak_note),
        frames(&l.momentary),
        frames(&l.short_term)
    )
}

fn crest_json(c: &Crest) -> String {
    let band = |b: &BandCrest| {
        format!(
            "\n    {{ \"lo_hz\": {}, \"hi_hz\": {}, \"centre_hz\": {}, \"peak\": {}, \
             \"rms\": {}, \"crest_db\": {}, \"counted\": {} }}",
            num(b.lo_hz),
            num(b.hi_hz),
            num(b.centre_hz),
            num(b.peak),
            num(b.rms),
            num(b.crest_db),
            b.counted
        )
    };
    format!(
        "{{ \"broadband_crest_db\": {}, \"spread_db\": {}, \"widest_band_hz\": {}, \
         \"tightest_band_hz\": {}, \"counted_under_db\": {}, \"bands\": {} }}",
        num(c.broadband_crest_db),
        maybe(c.spread_db),
        maybe(c.widest_band_hz),
        maybe(c.tightest_band_hz),
        num(c.counted_under_db),
        list(&c.bands, band)
    )
}

fn onsets_json(o: &Onsets) -> String {
    format!(
        "{{ \"onsets\": {}, \"resolution_secs\": {}, \"ioi_histogram\": {} }}",
        list(&o.onsets, |t| format!(
            "{{ \"t_secs\": {}, \"strength\": {} }}",
            num(t.t_secs),
            num(t.strength)
        )),
        num(o.resolution_secs),
        list(&o.ioi_histogram, |b| format!(
            "{{ \"lo_secs\": {}, \"hi_secs\": {}, \"count\": {} }}",
            num(b.lo_secs),
            num(b.hi_secs),
            b.count
        )),
    )
}

fn alias_json(a: &Alias) -> String {
    let band = |b: &AliasBand| {
        format!(
            "\n    {{ \"lo_hz\": {}, \"hi_hz\": {}, \"signal_db\": {}, \"alias_db\": {}, \
             \"nmr_db\": {} }}",
            num(b.lo_hz),
            num(b.hi_hz),
            num(b.signal_db),
            num(b.alias_db),
            num(b.nmr_db)
        )
    };
    format!(
        "{{ \"oversample\": {}, \"sample_rate\": {}, \"frame_size\": {}, \"frames\": {}, \
         \"scored_frames\": {}, \"playback_db_spl\": {}, \"asr_db\": {}, \"nmr_db\": {}, \
         \"nmr_peak_db\": {}, \"peak_at_secs\": {}, \"audible\": {}, \
         \"instances\": {}, \"bands\": {} }}",
        a.oversample,
        num(a.sample_rate),
        a.frame_size,
        a.frames,
        a.scored_frames,
        num(a.playback_db_spl),
        num(a.asr_db),
        num(a.nmr_db),
        num(a.nmr_peak_db),
        num(a.peak_at_secs),
        a.audible,
        a.instances,
        list(&a.bands, band)
    )
}

/// The fields every harmonic reading opens with: where its partials came from, the floor
/// under them, and the time axis its curve steps along.
fn framing_json(framing: &Framing, floor_db: f64) -> String {
    let window = framing.window.map_or(NONE.to_string(), |w| {
        format!(
            "{{ \"size\": {}, \"secs\": {}, \"resolution_hz\": {} }}",
            w.size,
            num(w.secs),
            num(w.resolution_hz)
        )
    });
    format!(
        "\"partials\": \"{}\", \"floor_db\": {}, \"hop_secs\": {}, \"window\": {window}",
        escape(framing.method()),
        num(floor_db),
        num(framing.hop_secs)
    )
}

fn extreme_json(e: Option<Extreme>) -> String {
    e.map_or(NONE.to_string(), |e| {
        format!(
            "{{ \"value\": {}, \"t\": {} }}",
            num(e.value),
            num(e.t_secs)
        )
    })
}

fn dissonance_json(d: &Dissonance) -> String {
    format!(
        "{{ \"model\": \"{}\", \"constants\": {{ \"a\": {}, \"b\": {}, \"d_star\": {}, \
         \"s1\": {}, \"s2\": {} }}, \"amplitude\": \"product\", {}, \"mean\": {}, \
         \"max\": {}, \"min\": {}, \"curve\": {} }}",
        escape(dissonance::MODEL),
        num(dissonance::A),
        num(dissonance::B),
        num(dissonance::D_STAR),
        num(dissonance::S1),
        num(dissonance::S2),
        framing_json(&d.framing, d.floor_db),
        maybe(d.mean),
        extreme_json(d.max),
        extreme_json(d.min),
        list(&d.frames, |f| format!(
            "\n    {{ \"t\": {}, \"roughness\": {}, \"roughness_bounds\": [{}, {}], \
             \"partials\": {} }}",
            num(f.t_secs),
            num(f.roughness),
            num(f.roughness_bounds.0),
            num(f.roughness_bounds.1),
            f.partials
        ))
    )
}

fn notes_json(notes: &[i64]) -> String {
    list(notes, |n| format!("\"{}\"", pitch_names::note_name(*n)))
}

fn chord_fields(c: &Chord) -> String {
    format!(
        "\"start_secs\": {}, \"end_secs\": {}, \"notes\": {}, \"unresolved\": {}",
        num(c.start_secs),
        num(c.end_secs),
        notes_json(&c.notes),
        notes_json(&c.unresolved)
    )
}

fn voice_leading_json(v: &VoiceLeading) -> String {
    format!(
        "{{ \"model\": \"{}\", \"norm\": \"{}\", \"chords_by\": \"{}\", {}, \
         \"total\": {}, \"chords\": {}, \"transitions\": {} }}",
        escape(voice_leading::MODEL),
        v.norm.key(),
        escape(chords::METHOD),
        framing_json(&v.framing, v.floor_db),
        num(v.total),
        list(&v.chords, |c| format!("\n    {{ {} }}", chord_fields(c))),
        list(&v.transitions, |t| format!(
            "\n    {{ \"t\": {}, \"from\": {}, \"to\": {}, \"distance\": {}, \
             \"mapping\": {} }}",
            num(t.at_secs),
            notes_json(&t.from),
            notes_json(&t.to),
            num(t.distance),
            list(&t.mapping, |(from, to)| format!(
                "{{ \"from\": \"{}\", \"to\": \"{}\" }}",
                pitch_names::note_name(*from),
                pitch_names::note_name(*to)
            ))
        ))
    )
}

fn triad_json(t: Option<Triad>) -> String {
    t.map_or(NONE.to_string(), |t| {
        format!(
            "{{ \"root\": \"{}\", \"quality\": \"{}\" }}",
            pitch_names::class_name(i64::from(t.root)),
            match t.quality {
                Quality::Major => "major",
                Quality::Minor => "minor",
            }
        )
    })
}

fn tonal_distance_json(d: &TonalDistance) -> String {
    let tonic = d.tonic.map_or(NONE.to_string(), |(triad, t)| {
        format!(
            "{{ \"triad\": {}, \"t\": {} }}",
            triad_json(Some(triad)),
            num(t)
        )
    });
    format!(
        "{{ \"model\": \"{}\", \"chords_by\": \"{}\", {}, \"tonic\": {tonic}, \
         \"chords\": {}, \"transitions\": {} }}",
        escape(tonnetz::MODEL),
        escape(chords::METHOD),
        framing_json(&d.framing, d.floor_db),
        list(&d.chords, |c| format!(
            "\n    {{ {}, \"triad\": {}, \"tension\": {} }}",
            chord_fields(&c.chord),
            triad_json(c.triad),
            counted(c.tension)
        )),
        list(&d.transitions, |t| format!(
            "\n    {{ \"t\": {}, \"from\": {}, \"to\": {}, \"distance\": {}, \
             \"path\": {} }}",
            num(t.at_secs),
            triad_json(t.from),
            triad_json(t.to),
            counted(t.path.as_ref().map(Vec::len)),
            t.path.as_ref().map_or(NONE.to_string(), |p| format!(
                "\"{}\"",
                p.iter().map(|s| s.letter()).collect::<String>()
            ))
        ))
    )
}

fn weighted_json(w: Option<Weighted>) -> String {
    w.map_or(NONE.to_string(), |w| {
        format!(
            "{{ \"value\": {}, \"bounds\": [{}, {}] }}",
            num(w.value),
            num(w.bounds.0),
            num(w.bounds.1)
        )
    })
}

fn bounds_json(b: Option<(f64, f64)>) -> String {
    b.map_or(NONE.to_string(), |(lo, hi)| {
        format!("[{}, {}]", num(lo), num(hi))
    })
}

fn series_json(s: &Series) -> String {
    let fit = s.fit.map_or(NONE.to_string(), |f| {
        format!(
            "{{ \"b\": {}, \"b_bounds\": {}, \"f0\": {}, \"f0_bounds\": {} }}",
            num(f.b),
            bounds_json(f.b_bounds),
            num(f.f0_hz),
            bounds_json(f.f0_bounds)
        )
    });
    format!(
        "{{ \"f1\": {}, \"cents\": {}, \"fit\": {fit}, \"members\": {} }}",
        num(s.members[0].partial.hz),
        weighted_json(s.cents),
        list(&s.members, |m| format!(
            "{{ \"n\": {}, \"hz\": {}, \"hz_bounds\": {}, \"amplitude\": {}, \
             \"amplitude_bounds\": {}, \"cents\": {}, \"cents_bounds\": {} }}",
            m.n,
            num(m.partial.hz),
            bounds_json(Some(m.partial.hz_bounds)),
            num(m.partial.amplitude),
            bounds_json(Some(m.partial.amplitude_bounds)),
            num(m.cents),
            bounds_json(Some(m.cents_bounds))
        ))
    )
}

fn inharmonicity_json(r: &Inharmonicity) -> String {
    format!(
        "{{ \"model\": \"{}\", {}, \"cents_mean\": {}, \"cents_max\": {}, \
         \"unexplained_mean\": {}, \"curve\": {} }}",
        escape(inharmonicity::MODEL),
        framing_json(&r.framing, r.floor_db),
        maybe(r.cents_mean),
        extreme_json(r.cents_max),
        maybe(r.unexplained_mean),
        list(&r.frames, |f| format!(
            "\n    {{ \"t\": {}, \"cents\": {}, \"unexplained\": {}, \"series\": {} }}",
            num(f.t_secs),
            weighted_json(f.cents),
            weighted_json(f.unexplained),
            list(&f.series, series_json)
        ))
    )
}

fn spread_json(s: Option<Stats>) -> String {
    s.map_or(NONE.to_string(), |s| {
        format!(
            "{{ \"mean\": {}, \"min\": {}, \"max\": {} }}",
            num(s.mean),
            extreme_json(Some(s.min)),
            extreme_json(Some(s.max))
        )
    })
}

/// `curve` is left out where not asked, and each of its heights null where the envelope is
/// constant.
fn periodicity_window_json(w: &Window) -> String {
    let curve = w.curve.as_ref().map_or(String::new(), |c| {
        let heights: Vec<String> = c.iter().map(|h| num(*h)).collect();
        format!(", \"curve\": [{}]", heights.join(", "))
    });
    format!(
        "\n      {{ \"t\": {}, \"from\": {}, \"to\": {}, \"drift_bpm\": {}, \"peaks\": {}{curve} }}",
        num(w.t_secs),
        num(w.from_secs),
        num(w.to_secs),
        maybe(w.drift_bpm),
        list(&w.peaks, |p| format!(
            "{{ \"period_secs\": {}, \"period_bounds_secs\": {}, \"bpm\": {}, \
             \"bpm_bounds\": {}, \"height\": {} }}",
            num(p.period_secs),
            bounds_json(p.period_bounds_secs),
            num(p.bpm),
            bounds_json(p.bpm_bounds),
            num(p.height)
        ))
    )
}

fn periodicity_track_json(t: &Track) -> String {
    let centres: Vec<String> = t.centres_hz.iter().map(|hz| num(*hz)).collect();
    format!(
        "\n    {{ \"group\": \"{}\", \"centres_hz\": [{}], \"summary\": {{ \
         \"windows_with_peak\": {}, \"bpm\": {}, \"height\": {}, \"drift_bpm\": {} }}, \
         \"windows\": {} }}",
        t.group.key(),
        centres.join(", "),
        t.summary.windows_with_peak,
        spread_json(t.summary.bpm),
        spread_json(t.summary.height),
        spread_json(t.summary.drift_bpm),
        list(&t.windows, periodicity_window_json)
    )
}

fn periodicity_json(p: &Periodicity) -> String {
    format!(
        "{{ \"model\": \"{}\", \"hop_secs\": {}, \"window_secs\": {}, \"envelope_hz\": {}, \
         \"lag_step_secs\": {}, \"min_period_secs\": {}, \"max_period_secs\": {}, \
         \"split_hz\": {}, \"smoothing\": {{ \"hz\": {}, \"poles\": {} }}, \"tracks\": {} }}",
        escape(periodicity::MODEL),
        num(p.hop_secs),
        num(p.window_secs),
        num(p.envelope_hz),
        num(p.lag_step_secs),
        num(p.min_period_secs),
        num(p.max_period_secs),
        num(periodicity::SPLIT_HZ),
        maybe(periodicity::SMOOTHING.hz),
        periodicity::SMOOTHING.poles,
        list(&p.tracks, periodicity_track_json)
    )
}

/// `period_beats` and its bounds are left out where no `bpm` was asked; `depth_key` names the
/// depth's unit.
fn modulation_peak_json(p: &modulation::Peak, depth_key: &str) -> String {
    let beats = p.period_beats.map_or(String::new(), |b| {
        format!(
            ", \"period_beats\": {}, \"period_beats_bounds\": {}",
            num(b),
            bounds_json(p.period_beats_bounds)
        )
    });
    format!(
        "{{ \"hz\": {}, \"hz_bounds\": {}{beats}, \"{depth_key}\": {}, \
         \"{depth_key}_bounds\": {} }}",
        num(p.hz),
        bounds_json(p.hz_bounds),
        num(p.depth),
        bounds_json(p.depth_bounds)
    )
}

fn modulation_window_json(w: &modulation::Window) -> String {
    let centroid = w.centroid.as_ref().map_or(NONE.to_string(), |c| {
        format!(
            "{{ \"mean_hz\": {}, \"peaks\": {} }}",
            num(c.mean_hz),
            list(&c.peaks, |p| modulation_peak_json(p, "depth_hz"))
        )
    });
    format!(
        "\n      {{ \"t\": {}, \"from\": {}, \"to\": {}, \"level\": {}, \"peaks\": {}, \
         \"centroid\": {centroid} }}",
        num(w.t_secs),
        num(w.from_secs),
        num(w.to_secs),
        num(w.level),
        list(&w.peaks, |p| modulation_peak_json(p, "depth"))
    )
}

fn modulation_track_json(t: &modulation::Track) -> String {
    let centres: Vec<String> = t.centres_hz.iter().map(|hz| num(*hz)).collect();
    format!(
        "\n    {{ \"group\": \"{}\", \"centres_hz\": [{}], \"summary\": {{ \
         \"windows_with_peak\": {}, \"hz\": {}, \"depth\": {}, \"centroid_hz\": {}, \
         \"centroid_depth_hz\": {} }}, \"windows\": {} }}",
        t.group.key(),
        centres.join(", "),
        t.summary.windows_with_peak,
        spread_json(t.summary.hz),
        spread_json(t.summary.depth),
        spread_json(t.summary.centroid_hz),
        spread_json(t.summary.centroid_depth_hz),
        list(&t.windows, modulation_window_json)
    )
}

fn modulation_json(m: &Modulation) -> String {
    let splits: Vec<String> = modulation::SPLITS_HZ.iter().map(|hz| num(*hz)).collect();
    format!(
        "{{ \"model\": \"{}\", \"hop_secs\": {}, \"window_secs\": {}, \"envelope_hz\": {}, \
         \"bin_step_hz\": {}, \"resolution_hz\": {}, \"min_hz\": {}, \"max_hz\": {}, \
         \"bpm\": {}, \"splits_hz\": [{}], \"smoothing\": {{ \"hz\": {}, \"poles\": {} }}, \
         \"passband_gain_at_max_hz\": {}, \"tracks\": {} }}",
        escape(modulation::MODEL),
        num(m.hop_secs),
        num(m.window_secs),
        num(m.envelope_hz),
        num(m.bin_step_hz),
        num(m.resolution_hz),
        num(m.min_hz),
        num(m.max_hz),
        maybe(m.bpm),
        splits.join(", "),
        maybe(modulation::SMOOTHING.hz),
        modulation::SMOOTHING.poles,
        num(m.passband_gain_at_max_hz),
        list(&m.tracks, modulation_track_json)
    )
}

fn span_json(span: (f64, f64)) -> String {
    format!("{{ \"from\": {}, \"to\": {} }}", num(span.0), num(span.1))
}

fn numbers(values: &[f64]) -> String {
    let held: Vec<String> = values.iter().map(|v| num(*v)).collect();
    format!("[{}]", held.join(", "))
}

fn self_similarity_cell_json(c: &self_similarity::Cell) -> String {
    format!(
        "\n    {{ \"t\": {}, \"from\": {}, \"to\": {}, \"chroma\": {}, \"levels_db\": {} }}",
        num(c.t_secs),
        num(c.from_secs),
        num(c.to_secs),
        numbers(&c.chroma),
        numbers(&c.levels_db)
    )
}

/// `matrix` is left out where not asked.
fn self_similarity_view_json(v: &self_similarity::Reading, from_secs: f64, step: f64) -> String {
    let matrix = v.matrix.as_ref().map_or(String::new(), |rows| {
        let rows: Vec<String> = rows
            .iter()
            .map(|r| format!("\n      {}", numbers(r)))
            .collect();
        format!(", \"matrix\": [{}]", rows.join(","))
    });
    format!(
        "\n    {{ \"view\": \"{}\", \"stripes_found\": {}, \"stripes\": {}, \
         \"novelty\": {{ \"from_secs\": {}, \"step_secs\": {}, \"values\": {} }}, \
         \"peaks_found\": {}, \"peaks\": {}{matrix} }}",
        v.view.key(),
        v.stripes_found,
        list(&v.stripes, |s| format!(
            "\n      {{ \"shift_semitones\": {}, \"lag_secs\": {}, \"earlier\": {}, \
             \"later\": {}, \"cells\": {}, \"similarity_mean\": {}, \"similarity_min\": {} }}",
            s.shift_semitones,
            num(s.lag_secs),
            span_json(s.earlier),
            span_json(s.later),
            s.cells,
            num(s.similarity_mean),
            num(s.similarity_min)
        )),
        num(from_secs),
        num(step),
        numbers(&v.novelty),
        v.peaks_found,
        list(&v.peaks, |p| format!(
            "{{ \"t\": {}, \"novelty\": {} }}",
            num(p.t_secs),
            num(p.novelty)
        ))
    )
}

/// `cells` is left out where not asked.
fn self_similarity_json(s: &SelfSimilarity) -> String {
    let cells = s.cells.as_ref().map_or(String::new(), |c| {
        format!(", \"cells\": {}", list(c, self_similarity_cell_json))
    });
    format!(
        "{{ \"model\": \"{}\", \"hop_secs\": {}, \"frame_window_secs\": {}, \
         \"resolution_hz\": {}, \"cell_secs\": {}, \"frames_per_cell\": {}, \
         \"cell_count\": {}, \"max_cells\": {}, \"smooth_cells\": {}, \"kernel_cells\": {}, \
         \"floor_db\": {}, \"listed\": {}, \"min_hz\": {}, \"max_hz\": {}, \
         \"centres_hz\": {}{cells}, \"views\": {} }}",
        escape(self_similarity::MODEL),
        num(s.hop_secs),
        num(s.frame_window_secs),
        num(s.resolution_hz),
        num(s.cell_secs),
        s.frames_per_cell,
        s.cell_count,
        self_similarity::MAX_CELLS,
        s.smooth_cells,
        s.kernel_cells,
        num(s.floor_db),
        s.listed,
        num(s.min_hz),
        num(s.max_hz),
        numbers(&s.centres_hz),
        list(&s.views, |v| self_similarity_view_json(
            v,
            s.novelty_from_secs,
            s.cell_secs
        ))
    )
}

fn binding_json(b: &Binding) -> String {
    format!(
        "\n    {{ \"name\": \"{}\", \"source\": \"{}\" }}",
        escape(&b.name),
        escape(&b.source)
    )
}

fn arguments_json(a: &Arguments) -> String {
    let at = |s: sva_ast::ByteSpan| format!("{{ \"start\": {}, \"end\": {} }}", s.start, s.end);
    let calls = list(&a.calls, |c| {
        let arguments = list(&c.arguments, |x| {
            format!(
                "{{ \"name\": \"{}\", \"value\": {}, \"written\": {} }}",
                escape(&x.name),
                num(x.value),
                x.written
            )
        });
        format!(
            "\n      {{ \"name\": \"{}\", \"at\": {}, \"arguments\": {arguments} }}",
            escape(&c.name),
            at(c.at)
        )
    });
    let chosen = list(&a.chosen, |c| {
        format!(
            "\n      {{ \"name\": \"{}\", \"at\": {}, \"operands\": {}, \"chosen\": {} }}",
            escape(&c.name),
            at(c.at),
            list(&c.operands, |v| num(*v)),
            c.chosen
        )
    });
    format!(
        "\n    {{ \"node\": \"{}\", \"calls\": {calls}, \"chosen\": {chosen} }}",
        escape(&a.node)
    )
}

/// One object per component: a buffer is planar, and the component IS the channel.
fn samples_json(b: &Buffer, limit: Option<usize>) -> String {
    let component = |c: usize| {
        let plane = b.plane(c);
        let shown = limit.map_or(plane.len(), |cap| plane.len().min(cap));
        format!(
            "\n    {{ \"channel\": {c}, \"values\": {} }}",
            capped(
                plane,
                shown,
                |n| b.origin_secs() + n as f64 / f64::from(b.rate),
                |v| num(*v)
            )
        )
    };
    let components: Vec<usize> = (0..b.width()).collect();
    format!(
        "{{ \"rate\": {}, \"origin_secs\": {}, \"width\": {}, \"components\": {} }}",
        b.rate,
        num(b.origin_secs()),
        b.width(),
        list(&components, |c| component(*c))
    )
}

fn symbolic_json(n: &SpectralSum) -> String {
    let terms: Vec<String> = n.atoms().map(sva_engine::sketch_atom).collect();
    format!(
        "{{ \"var\": \"{}\", \"lanes\": {}, \"terms\": {} }}",
        match n.var {
            sva_engine::Var::T => "t",
            sva_engine::Var::F => "f",
        },
        n.lanes.len(),
        crate::json::strings(&terms)
    )
}

/// `limit` caps the arrays a stdout reader scrolls past; `None` writes every value. `skim`
/// only changes the ledger.
pub fn value_json(output: &Output, limit: Option<usize>, skim: bool) -> String {
    match output {
        Output::Lines(lines) => list(lines, line_json),
        Output::Atoms(atoms) => list(atoms, atom_json),
        Output::Symbolic(sum) => symbolic_json(sum),
        Output::Samples(buffer) => samples_json(buffer, limit),
        Output::Ledger(entries) => ledger_json(entries, skim),
        Output::Spectrum(s) => spectrum_json(s),
        Output::Stereo(image) => stereo_json(image),
        Output::Bands(b) => bands_json(b, limit),
        Output::Loudness(l) => loudness_json(l, limit),
        Output::Crest(c) => crest_json(c),
        Output::Onsets(o) => onsets_json(o),
        Output::Alias(a) => alias_json(a),
        Output::Dissonance(d) => dissonance_json(d),
        Output::VoiceLeading(v) => voice_leading_json(v),
        Output::TonalDistance(d) => tonal_distance_json(d),
        Output::Inharmonicity(r) => inharmonicity_json(r),
        Output::Periodicity(p) => periodicity_json(p),
        Output::Modulation(m) => modulation_json(m),
        Output::SelfSimilarity(s) => self_similarity_json(s),
        Output::Bindings(b) => list(b, binding_json),
        Output::Arguments(a) => list(a, arguments_json),
        Output::Envelope(frames) => list(frames, |f: &EnvelopeFrame| {
            format!(
                "\n    {{ \"t\": {}, \"rms\": {}, \"peak\": {} }}",
                num(f.t_secs),
                num(f.rms),
                num(f.peak)
            )
        }),
        Output::Pitch(frames) => list(frames, |f| {
            format!(
                "\n    {{ \"t\": {}, \"notes\": {} }}",
                num(f.t_secs),
                list(&f.notes, |n| format!(
                    "{{ \"note\": \"{}\", \"hz\": {}, \"cents\": {}, \"db\": {}, \
                     \"harmonic_of\": {} }}",
                    escape(&n.name),
                    num(n.hz),
                    num(n.cents),
                    num(n.db),
                    maybe(n.harmonic_of)
                ))
            )
        }),
        Output::Formants(frames) => list(frames, formants_json),
    }
}

/// Each place on the sum's own axis in its unit, and the turn in the unit dual to it: in `t`
/// seconds and hertz, in `f` the reverse; `null` is a side the window leaves unbounded.
fn atom_json(a: &sva_engine::Atom) -> String {
    let (at, per, turns) = match a.var {
        sva_engine::Var::T => ("secs", "sec", "hz"),
        sva_engine::Var::F => ("hz", "hz", "secs"),
    };
    format!(
        "\n    {{ \"sketch\": \"{}\", \"{turns}\": {}, \"re\": {}, \"im\": {}, \"db\": {}, \
         \"decay_per_{per}\": {}, \"decay_from_{at}\": {}, \"degree\": {}, \
         \"polynomial_at_{at}\": {}, \"start_{at}\": {}, \"end_{at}\": {} }}",
        escape(&a.sketch),
        num(a.turns),
        num(a.amp.re),
        num(a.amp.im),
        num(20.0 * a.amp.abs().log10()),
        num(a.decay),
        num(a.decay_from),
        a.degree,
        num(a.polynomial_at),
        maybe(a.start),
        maybe(a.end)
    )
}

fn line_json(l: &sva_engine::Line) -> String {
    format!(
        "\n    {{ \"hz\": {}, \"re\": {}, \"im\": {}, \"db\": {} }}",
        num(l.hz),
        num(l.amp.re),
        num(l.amp.im),
        num(20.0 * l.amp.abs().log10())
    )
}

/// Every answer says which reading ran, under which profile, and at what rate.
/// A reading that truncated a series names what it left out, so the kept list is never read
/// as the whole of it.
pub fn answer_json(answer: &Answer, limit: Option<usize>, skim: bool) -> String {
    format!(
        "{{ \"source\": \"{}\", \"profile\": \"{}\", \"rate\": {}, \"value\": {}, \
         \"dropped\": {}, \"tail_db\": {} }}",
        match answer.source {
            Source::Exact => "exact",
            Source::Measured => "measured",
        },
        escape(answer.profile),
        answer
            .rate
            .map_or_else(|| NONE.to_string(), |r| r.to_string()),
        value_json(&answer.value, limit, skim),
        list(&answer.dropped, line_json),
        maybe(answer.tail_db)
    )
}

/// What one collapse row states beyond its name, as JSON fields.
fn detail_json(detail: &Detail) -> String {
    match detail {
        Detail::Lines {
            summed,
            dropped,
            dropped_more,
            tail_db,
            ..
        } => format!(
            ", \"summed\": {summed}, \"dropped\": {}, \"dropped_more\": {dropped_more}, \
             \"tail_db\": {}",
            list(dropped, |d| format!(
                "{{ \"hz\": {}, \"db\": {} }}",
                num(d.hz),
                num(d.db)
            )),
            maybe(*tail_db)
        ),
        Detail::Cropped { tail_db, .. } => format!(", \"tail_db\": {}", maybe(*tail_db)),
        Detail::Point {
            alias_db, tail_db, ..
        } => format!(
            ", \"alias_db\": {}, \"tail_db\": {}",
            maybe(*alias_db),
            maybe(*tail_db)
        ),
        Detail::Roundtrip { edited, .. } => format!(", \"edited\": {edited}"),
        Detail::Continuous { .. } | Detail::Reading { .. } => String::new(),
        Detail::Added { parts } => format!(
            ", \"addends\": {}",
            list(parts, |part| format!(
                "{{ \"rule\": \"{}\"{} }}",
                part.rule().as_str(),
                detail_json(part)
            ))
        ),
    }
}

/// The collapse label beside the reading it belongs to. `detail` is the one
/// place a key may be absent: each `rule` is its own shape, and its fields belong to it.
pub fn label_json(label: &Label) -> String {
    let detail = detail_json(&label.detail);
    let cutting = label
        .cutting_below_silence_threshold
        .as_ref()
        .map_or(NONE.to_string(), cutting_below_silence_threshold_json);
    let rest = format!(
        ", \"moved_s\": {}, \"pruned\": {cutting}",
        maybe(label.moved)
    );
    format!(
        "{{ \"source\": \"{}\", \"profile\": \"{}\", \"rate\": {}, \"rule\": \"{}\"{detail}{rest} }}",
        match label.source {
            Source::Exact => "exact",
            Source::Measured => "measured",
        },
        escape(label.profile),
        label.rate,
        escape(label.rule().as_str())
    )
}

/// The silence threshold, and each node treated as silent from a sample on, under the JSON
/// keys `pruned`, `db` and `cuts`.
pub fn cutting_below_silence_threshold_json(cutting: &CuttingBelowSilenceThreshold) -> String {
    let cuts = list(&cutting.treated_as_silent_from_sample, |(node, from)| {
        format!("{{ \"node\": \"{}\", \"from\": {from} }}", escape(node))
    });
    format!(
        "{{ \"db\": {}, \"cuts\": {cuts} }}",
        num(cutting.silence_threshold_dbfs)
    )
}

pub fn work_json(work: &Work) -> String {
    format!(
        "{{ \"samples\": {}, \"computed_samples\": {} }}",
        work.samples, work.computed_samples
    )
}

/// `computed` counts every miss, `stored` the misses memory kept and `replaced` the volatile
/// misses kept in place of their last value; memory's figures are as the render left it, and
/// `tier` what passed between memory and the disk meanwhile.
pub fn stats_json(stats: &CacheStats) -> String {
    stats_with(stats, "")
}

/// `stats_json` over a stream's latest lookups, `dropped`: the latest nodes its live edits
/// started silent, and `pruned`: where it cuts below the silence threshold. Each list's
/// `pagination.count` counts all it made.
pub fn stream_stats_json(
    stats: &CacheStats,
    (dropped, made): (&[&str], usize),
    cutting: &CuttingBelowSilenceThreshold,
) -> String {
    let dropped = latest(dropped, made, |name| format!("\"{}\"", escape(name)));
    let pruned = cutting_below_silence_threshold_json(cutting);
    stats_with(
        stats,
        &format!(", \"dropped\": {dropped}, \"pruned\": {pruned}"),
    )
}

fn stats_with(stats: &CacheStats, extra: &str) -> String {
    let made = stats.shed + stats.lookups.len();
    let lookups = latest(&stats.lookups, made, |l| {
        let outcome = match l.outcome {
            Outcome::Hit => "hit",
            Outcome::ComputedStored => "computed_stored",
            Outcome::ComputedNotStored => "computed_not_stored",
            Outcome::ComputedReplaced => "computed_replaced",
            Outcome::Extended => "extended",
            Outcome::Prefix => "prefix",
            Outcome::Reused => "reused",
        };
        format!(
            "{{ \"node\": \"{}\", \"key\": \"{}\", \"kind\": \"{}\", \"outcome\": \"{outcome}\" }}",
            escape(&l.node),
            l.key,
            match l.kind {
                PayloadKind::Segments => "segments",
                PayloadKind::Frames => "frames",
                PayloadKind::Run => "run",
            }
        )
    });
    format!(
        "{{ \"nodes\": {}, \"hits\": {}, \"computed\": {}, \"stored\": {}, \"replaced\": {}, \"extended\": {}, \"reused\": {}, \
         \"bytes\": {}, \"max_bytes\": {}, \"entries\": {}, \"evictions\": {}, \
         \"tier\": {}, \"lookups\": {lookups}{extra} }}",
        stats.nodes(),
        stats.hits(),
        stats.computed(),
        stats.stored(),
        stats.replaced(),
        stats.extended(),
        stats.reused(),
        stats.bytes,
        stats.max_bytes,
        stats.entries,
        stats.evictions,
        counters_json(&stats.tier)
    )
}

/// What passed between memory and the disk beneath it.
pub fn counters_json(counters: &Counters) -> String {
    format!(
        "{{ \"disk_lookups\": {}, \"disk_reads\": {}, \"disk_read_bytes\": {}, \
         \"promotions\": {}, \"writebacks\": {}, \"hits\": {}, \"evictions\": {}, \
         \"probation_evictions\": {}, \"protected_evictions\": {} }}",
        counters.disk_lookups,
        counters.disk_reads,
        counters.disk_read_bytes,
        counters.promotions,
        counters.writebacks,
        counters.hits,
        counters.evictions(),
        counters.probation_evictions,
        counters.protected_evictions
    )
}

fn worked_json(worked: &Worked) -> String {
    let lookups = worked.lookups;
    format!(
        "\"samples\": {}, \"computed_samples\": {}, \"lookups\": {{ \"hit\": {}, \
         \"from_memory\": {}, \"from_store\": {}, \"prefix\": {}, \"miss\": {}, \"new\": {}, \
         \"reused\": {} }}, \"tier\": {}",
        worked.samples,
        worked.computed_samples,
        lookups.hit,
        lookups.hit - lookups.from_store,
        lookups.from_store,
        lookups.prefix,
        lookups.miss,
        lookups.new,
        lookups.reused,
        counters_json(&worked.tier)
    )
}

/// One report of the engine-stats subscription, as `sva-wasm` declares `EngineStats`.
pub fn engine_stats_json(stats: &EngineStats) -> String {
    let call = stats.call.as_ref().map_or("null".to_string(), |call| {
        let kind = match call.kind {
            Kind::Render => "render",
            Kind::Stream => "stream",
            Kind::Edit => "edit",
            Kind::Persist => "persist",
        };
        let phase = match call.phase {
            Phase::Parsing => "parsing",
            Phase::Typing => "typing",
            Phase::LookingUp => "looking_up",
            Phase::Planning => "planning",
            Phase::Fetching => "fetching",
            Phase::Delivering => "delivering",
            Phase::Finishing => "finishing",
            Phase::Storing => "storing",
            Phase::Done => "done",
            Phase::Failed => "failed",
            Phase::Abandoned => "abandoned",
        };
        let total = call
            .total_samples
            .map_or("null".to_string(), |total| total.to_string());
        format!(
            "{{ \"id\": {}, \"kind\": \"{kind}\", \"target\": \"{}\", \"phase\": \"{phase}\", \
             \"total_samples\": {total}, {} }}",
            call.id,
            escape(&call.target),
            worked_json(&call.counts)
        )
    });
    let state = match stats.working {
        0 => "idle",
        _ => "working",
    };
    format!(
        "{{ \"state\": \"{state}\", \"working\": {}, \"call\": {call}, \"totals\": {{ {} }} }}",
        stats.working,
        worked_json(&stats.totals)
    )
}

/// One answer as a report prints it.
pub struct Printed {
    pub name: String,
    pub answer: Answer,
    /// `ledger(skim=1)`: each row's level alone.
    pub skim: bool,
}

/// What a render answered with, and where anything too big for the object went instead.
pub struct Report<'a> {
    pub target: &'a str,
    pub rate: u32,
    /// The precision every sample was written to; `None` for a file read back.
    pub bits: Option<i32>,
    /// The seconds the readings were taken over; `None` where none read samples.
    pub interval: Option<(f64, f64)>,
    pub profile: &'a str,
    pub label: Option<&'a Label>,
    pub written: &'a [(String, &'a Path)],
    /// Absent where no reading named a live device.
    pub played: Option<&'a Played>,
    pub answers: &'a [Printed],
    pub limit: Option<usize>,
}

/// What a live device played, as its player reported it back.
#[derive(Clone, Debug, PartialEq)]
pub struct Played {
    pub representation: String,
    /// As the destination named it, `alsa:hw:1`.
    pub device: String,
    pub seconds: f64,
    pub rate: u32,
    pub channels: usize,
    /// The ALSA sample format the samples were sent in, `FLOAT_LE`.
    pub format: &'static str,
    pub buffer_secs: f64,
    /// What the device gave for the buffer asked; `None` where the player stated none.
    pub granted_buffer_secs: Option<f64>,
    pub underruns: usize,
    /// What saw the underruns `underruns` counts, and what it cannot see.
    pub underruns_seen_by: &'static str,
}

fn played_json(played: &Played) -> String {
    format!(
        "{{ \"representation\": \"{}\", \"device\": \"{}\", \"seconds\": {}, \
         \"sample_rate\": {}, \"channels\": {}, \"format\": \"{}\", \"buffer\": {{ \
         \"asked_secs\": {}, \"granted_secs\": {} }}, \"underruns\": {{ \"count\": {}, \
         \"seen_by\": \"{}\" }} }}",
        escape(&played.representation),
        escape(&played.device),
        num(played.seconds),
        played.rate,
        played.channels,
        played.format,
        num(played.buffer_secs),
        maybe(played.granted_buffer_secs),
        played.underruns,
        escape(played.underruns_seen_by),
    )
}

/// `written` names every reading that went to a file rather than into `representations`.
pub fn query_data(report: &Report) -> String {
    let written = list(report.written, |(name, path)| {
        format!(
            "{{ \"representation\": \"{}\", \"path\": \"{}\" }}",
            escape(name),
            escape(&path.display().to_string())
        )
    });
    let label = report
        .label
        .map(label_json)
        .unwrap_or_else(|| NONE.to_string());
    let reads = report
        .answers
        .iter()
        .map(|printed| {
            format!(
                "\"{}\": {}",
                escape(&printed.name),
                answer_json(&printed.answer, report.limit, printed.skim)
            )
        })
        .collect::<Vec<_>>()
        .join(",\n    ");
    let interval = report.interval.map_or(NONE.to_string(), |(start, end)| {
        format!(
            "{{ \"start_secs\": {}, \"end_secs\": {} }}",
            num(start),
            num(end)
        )
    });
    let played = report.played.map_or(String::new(), |played| {
        format!("\n  \"played\": {},", played_json(played))
    });
    format!(
        "{{\n  \"target\": \"{}\",\n  \"sample_rate\": {},\n  \"bits\": {},\n  \
         \"profile\": \"{}\",\n  \"interval\": {interval},\n  \"label\": {label},\n  \
         \"written\": {written},{played}\n  \"representations\": {{\n    {reads}\n  }}\n}}",
        escape(report.target),
        report.rate,
        report.bits.map_or(NONE.to_string(), |b| b.to_string()),
        escape(report.profile),
    )
}
