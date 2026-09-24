// Concern: writes one reading and the render around it as the `data` object | Non-concern: the error envelope (output.rs), subcommand JSON (sva-cli) | IO: (Answer) -> a JSON string

use std::path::Path;

use sva_engine::{
    Alias, AliasBand, Answer, BandCrest, BandTrack, Bands, Binding, Buffer, CacheStats, Cost,
    Crest, Detail, EnvelopeFrame, FormantFrame, Horizon, Label, LedgerEntry, Loudness,
    LoudnessFrame, Outcome, Output, PayloadKind, Source, SpectralSum, Spectrum, StereoFrame,
    StereoImage, Tier,
};

use crate::json::{NONE, capped, escape, list, num};

/// Past this a caller reading stdout wants a narrower `--from`/`--to`, not a wall of JSON.
pub const SAMPLE_LIMIT: usize = 4096;

/// The store at `dir`, and every lookup the render made of it.
pub struct CacheReport {
    pub dir: String,
    pub stats: CacheStats,
    pub held_bytes: u64,
    pub max_bytes: u64,
    pub evicted_bytes: u64,
    pub faults: u64,
}

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

/// `rate_dependent` is what stops the figure reading as pure alias: a sampled loop is a
/// different signal at the oversampled rate.
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
         \"nmr_peak_db\": {}, \"peak_at_secs\": {}, \"audible\": {}, \"rate_dependent\": {}, \
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
        a.rate_dependent,
        a.instances,
        list(&a.bands, band)
    )
}

fn binding_json(b: &Binding) -> String {
    format!(
        "\n    {{ \"name\": \"{}\", \"source\": \"{}\" }}",
        escape(&b.name),
        escape(&b.source)
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
                |n| b.origin_secs + n as f64 / f64::from(b.rate),
                |v| num(*v)
            )
        )
    };
    let components: Vec<usize> = (0..b.width).collect();
    format!(
        "{{ \"rate\": {}, \"origin_secs\": {}, \"width\": {}, \"components\": {} }}",
        b.rate,
        num(b.origin_secs),
        b.width,
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

/// A cost tree, deepest row last, each already folded to the share it is worth printing.
fn flops_json(tree: &sva_engine::FlopTree) -> String {
    format!(
        "{{ \"total\": {}, \"budget\": {}, \"rows\": {} }}",
        tree.total,
        tree.budget,
        list(&tree.rows, |r: &sva_engine::FlopRow| format!(
            "\n    {{ \"depth\": {}, \"node\": \"{}\", \"own\": {}, \"subtree\": {}, \
             \"percent\": {}, \"route\": \"{}\", \"shared\": {} }}",
            r.depth,
            escape(&r.node),
            r.own,
            r.subtree,
            num(r.percent),
            escape(r.route),
            r.shared
        ))
    )
}

/// `limit` caps the arrays a stdout reader scrolls past; `None` writes every value. `skim`
/// only changes the ledger.
pub fn value_json(output: &Output, limit: Option<usize>, skim: bool) -> String {
    match output {
        Output::Lines(lines) => list(lines, line_json),
        Output::Atoms(sketches) => crate::json::strings(sketches),
        Output::Symbolic(sum) => symbolic_json(sum),
        Output::Samples(buffer) => samples_json(buffer, limit),
        Output::Ledger(entries) => ledger_json(entries, skim),
        Output::Spectrum(s) => spectrum_json(s),
        Output::Stereo(image) => stereo_json(image),
        Output::Bands(b) => bands_json(b, limit),
        Output::Loudness(l) => loudness_json(l, limit),
        Output::Crest(c) => crest_json(c),
        Output::Alias(a) => alias_json(a),
        Output::Bindings(b) => list(b, binding_json),
        Output::Flops(tree) => flops_json(tree),
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

fn line_json(l: &sva_engine::Line) -> String {
    format!(
        "\n    {{ \"hz\": {}, \"re\": {}, \"im\": {}, \"db\": {} }}",
        num(l.hz),
        num(l.amp.re),
        num(l.amp.im),
        num(20.0 * l.amp.abs().log10())
    )
}

/// FORMAT 14.3: every answer says which reading ran, under which profile, and at what rate.
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

/// What one row of the collapse table states beyond its name, as JSON fields.
fn detail_json(detail: &Detail) -> String {
    match detail {
        Detail::Lines {
            placed,
            summed,
            dropped,
            dropped_more,
            terms,
            tail_db,
            ..
        } => format!(
            ", \"placed\": {placed}, \"summed\": {summed}, \"dropped\": {}, \
             \"dropped_more\": {dropped_more}, \"terms\": {}, \"tail_db\": {}",
            list(dropped, |d| format!(
                "{{ \"hz\": {}, \"db\": {} }}",
                num(d.hz),
                num(d.db)
            )),
            counted(*terms),
            maybe(*tail_db)
        ),
        Detail::Cropped { tail_db, .. } => format!(", \"tail_db\": {}", maybe(*tail_db)),
        Detail::Point { alias_db, .. } => format!(", \"alias_db\": {}", maybe(*alias_db)),
        Detail::Spectrum { wrap_db, .. } => format!(", \"wrap_db\": {}", num(*wrap_db)),
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

/// The collapse label beside the reading it belongs to, per FORMAT 9.3. `detail` is the one
/// place a key may be absent: each `rule` is its own shape, and its fields belong to it.
pub fn label_json(label: &Label) -> String {
    let detail = detail_json(&label.detail);
    let cost = match label.cost {
        Some(Cost { flops, budget }) => format!(", \"flops\": {flops}, \"flop_budget\": {budget}"),
        None => format!(", \"flops\": {NONE}, \"flop_budget\": {NONE}"),
    };
    format!(
        "{{ \"source\": \"{}\", \"profile\": \"{}\", \"rate\": {}, \"rule\": \"{}\"{detail}{cost} }}",
        match label.source {
            Source::Exact => "exact",
            Source::Measured => "measured",
        },
        escape(label.profile),
        label.rate,
        escape(label.rule().as_str())
    )
}

fn cache_json(cache: Option<&CacheReport>) -> String {
    match cache {
        Some(c) => format!(
            "{{ \"dir\": \"{}\", \"held_bytes\": {}, \"max_bytes\": {}, \
             \"evicted_bytes\": {}, \"faults\": {}, \"stats\": {} }}",
            escape(&c.dir),
            c.held_bytes,
            c.max_bytes,
            c.evicted_bytes,
            c.faults,
            stats_json(&c.stats)
        ),
        None => NONE.to_string(),
    }
}

/// `computed` counts every miss, `stored` the misses the store kept; a lookup's `tier` is
/// written only on a hit.
pub fn stats_json(stats: &CacheStats) -> String {
    let lookups = list(&stats.lookups, |l| {
        let (outcome, tier) = match l.outcome {
            Outcome::Hit(tier) => ("hit", format!(", \"tier\": \"{}\"", tier_name(tier))),
            Outcome::ComputedStored => ("computed_stored", String::new()),
            Outcome::ComputedNotStored => ("computed_not_stored", String::new()),
        };
        format!(
            "{{ \"node\": \"{}\", \"key\": \"{}\", \"kind\": \"{}\", \"outcome\": \"{outcome}\"{tier} }}",
            escape(&l.node),
            l.key,
            match l.kind {
                PayloadKind::Samples => "samples",
                PayloadKind::Frames => "frames",
                PayloadKind::Symbolic => "symbolic",
            }
        )
    });
    format!(
        "{{ \"nodes\": {}, \"hits\": {{ \"memory\": {}, \"persistent\": {} }}, \
         \"computed\": {}, \"stored\": {}, \"lookups\": {lookups} }}",
        stats.nodes(),
        stats.hits_in(Tier::Memory),
        stats.hits_in(Tier::Persistent),
        stats.computed(),
        stats.stored()
    )
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Memory => "memory",
        Tier::Persistent => "persistent",
    }
}

/// What a render answered with, and where anything too big for the object went instead.
pub struct Report<'a> {
    pub target: &'a str,
    pub rate: u32,
    pub horizon: Horizon,
    pub profile: &'a str,
    pub label: Option<&'a Label>,
    pub written: &'a [(String, &'a Path)],
    pub cache: Option<&'a CacheReport>,
    pub answers: &'a [(String, Answer)],
    /// Readings a crate outside this pipeline answered, each already a JSON value: this
    /// envelope only says which reading ran, under which profile, and at what rate.
    pub analyses: &'a [(String, String)],
    pub limit: Option<usize>,
    pub skim: bool,
}

/// `written` names every reading that went to a file rather than into this object.
pub fn query_data(report: &Report) -> String {
    let written = list(report.written, |(name, path)| {
        format!(
            "{{ \"as\": \"{}\", \"path\": \"{}\" }}",
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
        .map(|(name, answer)| {
            format!(
                "\"{}\": {}",
                escape(name),
                answer_json(answer, report.limit, report.skim)
            )
        })
        .chain(report.analyses.iter().map(|(name, value)| {
            format!(
                "\"{}\": {{ \"source\": \"measured\", \"profile\": \"{}\", \"rate\": {}, \
                 \"value\": {value} }}",
                escape(name),
                escape(report.profile),
                report.rate
            )
        }))
        .collect::<Vec<_>>()
        .join(",\n  ");
    let tail = match reads.is_empty() {
        true => String::new(),
        false => format!(",\n  {reads}"),
    };
    format!(
        "{{\n  \"target\": \"{}\",\n  \"sample_rate\": {},\n  \"profile\": \"{}\",\n  \
         \"window\": {{ \"start_secs\": {}, \"end_secs\": {} }},\n  \"label\": {label},\n  \
         \"written\": {written},\n  \"cache\": {}{tail}\n}}",
        escape(report.target),
        report.rate,
        escape(report.profile),
        num(report.horizon.start_secs),
        num(report.horizon.end_secs),
        cache_json(report.cache)
    )
}
