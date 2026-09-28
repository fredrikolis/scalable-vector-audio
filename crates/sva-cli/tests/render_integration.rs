// Concern: exercises sva-cli's probe/answer/write end to end over fixtures | Non-concern: unit-level parsing or collapse math | IO: (fixtures/*) -> asserted Rendered, JSON or CliError

mod helpers;
mod tempo;
mod written;

use helpers::{asked, entry, fixture, json_of, master, plane, put, scratch, secs};
use sva_ast::Dir;
use sva_cli::{CliError, error_envelope};
use sva_core::{Job, PROBE, ROOT, execute, probe};
use sva_engine::{Cache, Output, PayloadKind, Representation, Source};

/// `expr` over its first second, read through a node of its own.
fn second(expr: &str) -> sva_core::Rendered {
    let dir = scratch("second");
    put(&dir, "x", &format!("{expr}\n"));
    probe(&dir, "@x([0, 1s])").unwrap_or_else(|e| panic!("`{expr}`: {e}"))
}

#[test]
fn basic_fixture_renders_with_bpm_meter_and_a_repeat_desugared() {
    let rendered = master(&fixture("basic")).expect("basic fixture should render");
    assert_eq!(secs(&rendered), 1.0, "the interval's own end");
    assert!(!plane(&rendered, PROBE).is_empty());

    let root = entry(&rendered, PROBE);
    assert_eq!(root.share, Some(1.0), "a node's share of itself is 1.0");
    assert_eq!(root.depth, 0);
    assert_eq!(root.clipped, Some(false));
    assert!(root.peak > 0.0 && root.peak < 1.0);
}

/// An open interval ends where the root's support does: an arrangement's desugared `concat`
/// crops its last section, so it ends where that section does.
#[test]
fn an_arranged_root_ends_where_its_last_section_does() {
    let rendered = probe(&fixture("arranged"), "@master").expect("arranged fixture should render");
    assert_eq!(secs(&rendered), 6.0, "1 bar + 2 bars at 120bpm 4/4");
    let samples = plane(&rendered, PROBE);
    assert_eq!(samples.len(), 6 * 44100);
    assert!(
        samples[5 * 44100..].iter().any(|s| *s != 0.0),
        "the last section sounds"
    );
}

/// The rate is an observation parameter: the same seconds on a different grid, with nothing
/// in the expression able to read which grid it landed on.
#[test]
fn a_stated_sample_rate_lays_the_same_seconds_on_a_different_grid() {
    let dir = fixture("arranged");
    let source = Dir::at(&dir);
    let at = |hz: Option<u32>| {
        execute(Job {
            rate: hz,
            ..Job::over(&source, "@drop-2b([0, 2b])")
        })
        .unwrap()
    };
    let base = at(None);
    let fine = at(Some(96_000));
    assert_eq!(base.config.rate, 44_100, "unstated is the default");
    assert_eq!(fine.config.rate, 96_000);
    assert_eq!(secs(&base), secs(&fine), "the same seconds either way");
    assert_eq!(plane(&base, PROBE).len(), 4 * 44_100);
    assert_eq!(plane(&fine, PROBE).len(), 4 * 96_000);
}

#[test]
fn no_tempo_fixture_skips_the_bridge_and_still_renders() {
    let rendered = master(&fixture("no-tempo")).expect("no-tempo fixture should render");
    assert_eq!(secs(&rendered), 1.0);
    assert_eq!(entry(&rendered, PROBE).share, Some(1.0));
}

#[test]
fn a_ledger_envelope_names_its_target_window_and_every_node_under_it() {
    let rendered = master(&fixture("basic")).unwrap();
    let json = json_of(&rendered, "ledger", Representation::Ledger { depth: 8 });

    assert!(json.contains("\"status\": \"success\""));
    assert!(json.contains("\"target\": \"@master([0, 1s])\""));
    assert!(json.contains("\"sample_rate\": 44100"));
    assert!(json.contains("\"start_secs\": 0"));
    assert!(json.contains("\"end_secs\": 1"));
    assert!(json.contains("\"written\": { \"items\": [], \"pagination\""));
    assert!(json.contains("\"node\": \"probe\""));
    for field in [
        "\"unit\"",
        "\"control\"",
        "\"rms\"",
        "\"peak\"",
        "\"share\"",
    ] {
        assert!(json.contains(field), "missing {field}");
    }
    assert!(json.contains("\"meta\"") && json.contains("\"timestamp\""));
    assert!(!json.contains("\"spectrum\""), "one query, one answer");
}

/// FORMAT 14.3: a reading off a closed form is exact and rate-free, one off a buffer is measured at
/// the rate it ran at, and both name the profile they were taken under.
#[test]
fn every_answer_names_its_source_profile_and_rate() {
    let rendered = second("sin(2*pi*440*t)");

    let law = rendered.answer(PROBE, Representation::Lines).unwrap();
    assert_eq!(law.source, Source::Exact);
    assert_eq!(law.profile, "psychoacoustic-v1");
    assert_eq!(law.rate, None, "a law answers without a grid");
    let Output::Lines(lines) = &law.value else {
        panic!("a pair answers lines")
    };
    assert!(lines.iter().any(|l| (l.hz - 440.0).abs() < 1e-9));

    let measured = rendered.answer(PROBE, Representation::Loudness).unwrap();
    assert_eq!(measured.source, Source::Measured);
    assert_eq!(measured.rate, Some(44_100));

    let json = json_of(&rendered, "lines", Representation::Lines);
    assert!(json.contains("\"source\": \"exact\""));
    assert!(json.contains("\"profile\": \"psychoacoustic-v1\""));
    assert!(json.contains("\"rate\": null"));
}

/// The point of probing: an effect is measured against the real graph without touching disk.
#[test]
fn probe_evaluates_an_argv_expression_in_the_compositions_namespace() {
    let dir = fixture("basic");
    let before = master(&dir).unwrap();
    let probed = probe(&dir, "crop(@drums*0.5, 0s, 1s)").expect("probe should render");

    assert_eq!(secs(&probed), secs(&before));
    let dry = entry(&before, PROBE).rms;
    let wet = entry(&probed, PROBE).rms;
    assert!(
        wet < dry,
        "half the level of the graph it read: {dry} -> {wet}"
    );
    assert!(wet > 0.0, "and not all of it");
    assert!(
        !dir.join(PROBE).exists(),
        "a probe writes nothing to the composition"
    );
}

#[test]
fn a_probe_naming_a_missing_node_or_a_broken_expression_refuses() {
    let dir = fixture("basic");
    let Err(missing) = probe(&dir, "@nope*2") else {
        panic!("no such node");
    };
    assert_eq!(missing.code(), "not_found");
    assert!(matches!(
        probe(&dir, "lp(@drums, cutoff="),
        Err(CliError::BadProbe(_))
    ));
}

/// A caller reading this JSON must be able to subtract the floor without a second call, so
/// every band carries its own beside the measurement it belongs to.
#[test]
fn a_bands_rendering_reports_every_bands_own_impulse_floor_beside_it() {
    let rendered = master(&fixture("basic")).unwrap();
    let json = json_of(&rendered, "bands", Representation::Bands);
    for field in [
        "\"rate_hz\"",
        "\"centre_hz\"",
        "\"erb_hz\"",
        "\"floor\"",
        "\"rise_10_90_secs\"",
    ] {
        assert!(json.contains(field), "missing {field}");
    }

    let Output::Bands(bands) = rendered.answer(PROBE, Representation::Bands).unwrap().value else {
        panic!("expected bands");
    };
    assert_eq!(bands.bands.len(), sva_engine::BAND_COUNT);
    let voiced = bands
        .bands
        .iter()
        .find(|b| (b.centre_hz - 220.0).abs() < 30.0)
        .expect("a band sits on the 220 Hz partial");
    assert!(voiced.peak > 0.0);
    assert!(voiced.floor.rise_10_90_secs.unwrap() > 0.0);
}

#[test]
fn error_envelope_carries_a_located_diagnostic_per_refusal() {
    let dir = scratch("refusal-json");
    put(&dir, "master", "@nope\n");

    let err = master(&dir).err().expect("a dangling ref refuses");
    let json = error_envelope(err.code(), &err.message(), &err.diagnostics());

    assert!(json.contains("\"status\": \"error\""));
    assert!(json.contains("\"code\": \"validation_error\""));
    assert!(json.contains("\"diagnostics\""));
    assert!(json.contains("\"dangling-ref\""));
    assert!(json.contains("\"location\""));
    assert!(json.contains("\"master\""));
}

/// The `cli` standard's envelope names three keys under `error`, and a missing `details`
/// leaves an agent branching on the code with nowhere structured to read the particulars.
#[test]
fn an_error_envelope_carries_code_message_and_details() {
    let dir = scratch("envelope-details");
    put(&dir, "master", "@nope\n");

    let err = master(&dir).err().expect("a dangling ref refuses");
    let json = error_envelope(err.code(), &err.message(), &err.diagnostics());
    let (error, data) = json
        .split_once("\"data\"")
        .expect("an error envelope holds both objects");

    for key in ["\"code\"", "\"message\"", "\"details\""] {
        assert!(error.contains(key), "`error` names {key}: {json}");
    }
    assert!(error.contains(err.code()), "the code branched on: {json}");
    for found in err.diagnostics() {
        let code = sva_core::json::escape(&found.code);
        assert!(error.contains(&code), "`details` names `{code}`: {json}");
        assert!(data.contains(&code), "and `data` locates it: {json}");
    }
    assert!(data.contains("\"location\""), "located: {json}");
}

#[test]
fn a_composition_directory_with_a_dangling_ref_refuses_not_panics() {
    let dir = scratch("dangling");
    put(&dir, "master", "@nope\n");
    assert!(matches!(master(&dir), Err(CliError::Refusals(_))));
}

/// Freeverb's twelve near-identical delay lines are two files and twelve invocations, each
/// an instance of its own with its own state.
#[test]
fn a_reverb_built_from_two_function_files_types_one_instance_per_invocation() {
    let graph = sva_core::prepared(&Dir::at(fixture("reverb"))).unwrap();
    let typing = sva_engine::types(&graph, ROOT).expect("the reverb types");
    let count = |prefix: &str| {
        typing
            .paths()
            .filter(|(p, _)| p.starts_with(prefix))
            .count()
    };
    assert_eq!(count("fx/comb("), 8, "eight comb invocations");
    assert_eq!(count("fx/allpass("), 4, "four allpass invocations");
}

/// sva-engine owns which width faults exist; what only this layer states is that one becomes
/// a single diagnostic carrying the file and byte span.
#[test]
fn a_width_conflict_refuses_the_whole_render_as_a_located_diagnostic() {
    let dir = scratch("width");
    put(&dir, "master", "crop(join(t, t) + join(t, t, t), 0s, 1s)\n");

    let err = master(&dir).err().expect("a width conflict refuses");
    let [diagnostic] = err.diagnostics().try_into().ok().unwrap();
    assert_eq!(diagnostic.code, "type.width_mismatch");
    assert_eq!(diagnostic.file.as_deref(), Some("master"));
    assert!(diagnostic.message.contains("2 components meet 3"));
}

/// The peak figure is the one thing this rendering cannot measure to standard, so it says so
/// in the report rather than leaving a reader to assume a true-peak reading.
#[test]
fn a_loudness_rendering_names_its_peak_as_a_sample_peak() {
    let rendered = master(&fixture("basic")).unwrap();
    let json = json_of(&rendered, "loudness", Representation::Loudness);
    assert!(json.contains("\"integrated_lufs\""), "{json}");
    assert!(json.contains("\"range_lu\""));
    assert!(json.contains("\"sample_peak_dbfs\""));
    assert!(
        json.contains("sample peak, not true peak"),
        "the report must state what the peak is not"
    );
    assert!(!json.contains("true_peak\""), "no field claims a true peak");
}

#[test]
fn a_crest_rendering_reports_the_spread_and_which_bands_it_counted() {
    let rendered = master(&fixture("basic")).unwrap();
    let json = json_of(&rendered, "crest", Representation::Crest);
    assert!(json.contains("\"spread_db\""), "{json}");
    assert!(json.contains("\"widest_band_hz\""));
    assert!(json.contains("\"tightest_band_hz\""));
    assert!(json.contains("\"crest_db\"") && json.contains("\"counted\""));
}

/// A point-sampled nonlinearity is the case the measure exists for; a pair collapsed off its
/// own line spectrum is the case it must not cry wolf on.
#[test]
fn an_alias_rendering_separates_a_folding_law_from_one_that_does_not() {
    let measured = |expr: &str| {
        let rendered = second(expr);
        match rendered
            .answer(PROBE, Representation::Alias { oversample: 4 })
            .expect("alias measures")
            .value
        {
            Output::Alias(a) => *a,
            other => panic!("expected an alias, got {other:?}"),
        }
    };

    let clean = measured("sin(2*pi*440*t)");
    assert!(!clean.audible, "{} dB NMR", clean.nmr_db);
    assert!(clean.asr_db < -100.0, "{} dB ASR", clean.asr_db);
    assert_eq!(clean.oversample, 4);

    let folding = measured("tanh(sin(2*pi*4000*t)*8)");
    assert!(folding.audible, "{} dB NMR", folding.nmr_db);
    assert!(
        folding.asr_db > clean.asr_db + 80.0,
        "{} against {}",
        folding.asr_db,
        clean.asr_db
    );
    assert!(folding.scored_frames > 0 && folding.bands.len() > 20);

    let exact = measured("saw(4000)");
    assert!(
        !exact.audible,
        "a line spectrum places every partial: {} dB NMR",
        exact.nmr_db
    );
}

/// `t = 0` is the transient and what a `crop` declares before it is buildup, so a swoosh
/// written at negative `t` reaches a hit placed later without anyone nudging it, and a hit
/// read alone starts where its buildup does.
#[test]
fn a_declared_pre_roll_renders_before_zero_and_an_undeclared_one_stays_silent() {
    let dir = scratch("preroll");
    put(&dir, "swoosh", "sin(2*pi*300*t)\n");
    put(&dir, "hit", "crop(@swoosh(t), -0.5s, 0.5s)\n");
    put(&dir, "master", "crop(@hit(t - 1s), 0s, 2s)\n");

    let rendered = probe(&dir, "@hit").expect("the composition renders");
    let range = rendered.render.range.expect("a range");
    assert_eq!(
        range.start, -22_050,
        "a range starts where a crop reaches before zero"
    );
    let rendered = probe(&dir, "@master").expect("the composition renders");
    let played = plane(&rendered, PROBE);
    let energy = |from: f64, to: f64| {
        let at = |s: f64| (s * 44100.0) as usize;
        played[at(from)..at(to)].iter().map(|s| s * s).sum::<f64>()
    };
    assert!(energy(0.6, 0.9) > 1.0, "the buildup arrives before the hit");
    assert!(
        energy(0.1, 0.4) < 1e-9,
        "and nothing reaches further back than declared"
    );

    put(&dir, "hit", "crop(@swoosh(t), 0s, 0.5s)\n");
    let plain = probe(&dir, "@master").expect("the composition renders");
    assert_eq!(plain.render.range.expect("a range").start, 0);
    let quiet = plane(&plain, PROBE);
    assert!(
        quiet[..44100].iter().map(|s| s * s).sum::<f64>() < 1e-9,
        "reaching before zero is never inferred"
    );
}

/// A page holds a registry, not a directory. Only what the target reaches is pulled, so a
/// broken node nothing reaches is never read.
#[test]
fn a_render_pulls_only_the_closure_its_target_reaches() {
    let mut held = sva_ast::Composition::new();
    held.insert("variables/bpm", "120\n");
    held.insert("variables/meter", "4/4\n");
    held.insert("lead-2b", "sin(2*pi*220*t)\n");
    held.insert("master", "@lead-2b*0.5\n");
    held.insert("nothing-reaches-this", "this is not an expression (\n");
    let reached = execute(Job::over(&held, "@lead-2b([0, 2b])"))
        .expect("only what `lead-2b` reaches is read");
    assert_eq!(secs(&reached), 4.0, "two bars at 120bpm");
}

/// A closed form answers `lines` off its own spectral sum, and no buffer is allocated behind it.
#[test]
fn a_law_answers_lines_with_no_buffer_behind_it() {
    let dir = fixture("basic");
    let source = Dir::at(&dir);
    let rendered = execute(Job {
        asked: &asked("lines"),
        ..Job::over(&source, "sin(2*pi*261.63*t) + sin(2*pi*329.63*t)")
    })
    .unwrap();
    assert!(rendered.render.range.is_none(), "and reads no range");
    assert!(
        rendered.render.buffers.is_empty(),
        "a law reading allocates nothing"
    );
    let Output::Lines(lines) = rendered.answer(PROBE, Representation::Lines).unwrap().value else {
        panic!("expected lines")
    };
    let mut hz: Vec<f64> = lines.iter().map(|l| l.hz.abs()).collect();
    hz.sort_by(f64::total_cmp);
    hz.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    assert_eq!(hz.len(), 2, "two partials, each with its mirror: {hz:?}");
}

/// FORMAT 14.1: `lines` is the exact line list of a pair, whichever variable the closed form was
/// written in — a turning exponential in `t`, and the delta it duals to in `f`.
#[test]
fn lines_reads_a_pair_written_in_either_variable() {
    let dir = fixture("basic");
    let listed = |expr: &str| {
        let source = Dir::at(&dir);
        let rendered = execute(Job {
            asked: &asked("lines"),
            ..Job::over(&source, expr)
        })
        .expect("the probe types");
        let Output::Lines(lines) = rendered.answer(PROBE, Representation::Lines).unwrap().value
        else {
            panic!("expected lines")
        };
        let mut hz: Vec<f64> = lines.iter().map(|l| l.hz).collect();
        hz.sort_by(f64::total_cmp);
        hz
    };
    assert_eq!(
        listed("delta(f - 185) + 0.6*delta(f - 331)"),
        vec![185.0, 331.0]
    );
    assert_eq!(listed("cos(2*pi*185*t)"), vec![-185.0, 185.0]);
}

/// FORMAT 14: one render, many readings. Two readings off one buffer collapse it once, so the
/// render looks that one buffer up once rather than once per representation.
#[test]
fn two_readings_share_one_collapse() {
    let dir = scratch("two-readings");
    put(
        &dir,
        "master",
        "; Models: a stack | Neglects: an envelope | IO: (t) -> amplitude | Tags: test\n\
         sin(2*pi*100*t) + sin(2*pi*200*t)\n",
    );
    let cache = Cache::new();
    let source = Dir::at(&dir);
    let rendered = execute(Job {
        asked: &asked("loudness, envelope"),
        cache: Some(&cache),
        ..Job::over(&source, "@master([0, 1s])")
    })
    .expect("the stack renders");
    let stats = rendered
        .render
        .cache_stats
        .expect("a render handed a store");
    let buffers = stats
        .lookups
        .iter()
        .filter(|l| l.kind == PayloadKind::Samples)
        .count();
    assert_eq!(
        buffers, 1,
        "two readings collapse the one node once: {stats:?}"
    );
}

/// Asking for a node the graph does not hold is a missing resource, not a malformed request.
#[test]
fn a_missing_node_is_not_found() {
    let dir = scratch("absent-node");
    put(&dir, "master", "sin(2*pi*300*t)\n");

    let source = Dir::at(&dir);
    let rendered = execute(Job {
        asked: &asked("lines"),
        ..Job::over(&source, "@master")
    })
    .expect("the composition renders");
    let Err(refused) = rendered.answer("nowhere", Representation::Lines) else {
        panic!("a node nothing defines has no reading");
    };
    assert_eq!(refused.code(), "not_found");
    assert_eq!(refused.exit_code(), 24);
}

/// One meaning of "missing" per tool, per the `cli` standard: every key an object declares is
/// written and answers `null` where there is no value, rather than being left out.
#[test]
fn every_absent_reading_is_null_never_a_missing_key() {
    let dir = fixture("basic");
    let rendered = master(&dir).expect("basic renders");
    let json = json_of(&rendered, "ledger", Representation::Ledger { depth: 8 });
    assert!(
        json.contains("\"channel\": null"),
        "a mono row still names its channel: {json}"
    );
    assert!(
        json.contains("\"tail_db\": null"),
        "an answer that dropped nothing still names its tail: {json}"
    );

    let traced = sva_cli::trace_data(&sva_cli::trace(&dir, "drums").expect("drums traces"));
    for absent in ["\"discrete\": null", "\"file\": null", "\"loop\": null"] {
        assert!(
            traced.contains(absent),
            "a trace writes {absent} rather than dropping the key: {traced}"
        );
    }
}

/// One path, one reading. Two readings at one destination would leave one of them on disk
/// while the envelope named both as written.
#[test]
fn two_readings_at_one_destination_refuse_before_anything_is_rendered() {
    let argv = |parts: &[&str]| -> Vec<String> { parts.iter().map(|s| (*s).to_string()).collect() };
    let shared = sva_cli::parse_args(&argv(&[
        "render",
        "@a",
        "--representation",
        "lines=/tmp/one.json,atoms=/tmp/one.json",
    ]));
    let Err(refused) = shared else {
        panic!("one path cannot hold two readings");
    };
    assert_eq!(refused.code(), "validation_error");
    assert!(refused.message().contains("/tmp/one.json"), "{refused}");

    assert!(
        sva_cli::parse_args(&argv(&[
            "render",
            "@a",
            "--representation",
            "lines=/tmp/one.json",
            "--representation",
            "atoms=/tmp/two.json",
        ]))
        .is_ok(),
        "two paths are two readings"
    );
    assert!(
        sva_cli::parse_args(&argv(&["render", "@a", "--representation", "lines,atoms"])).is_ok(),
        "and stdout holds both under their own keys"
    );
    assert!(
        sva_cli::parse_args(&argv(&[
            "render",
            "@a",
            "--representation",
            "lines=one.json,atoms=./one.json",
        ]))
        .is_err(),
        "`./one.json` and `one.json` are one file"
    );

    // `analyze` builds its destinations from two lists: the representations and the analyses.
    let mixed = sva_cli::parse_args(&argv(&[
        "analyze",
        "in.wav",
        "--representation",
        "spectrum=/tmp/both.json,onsets=/tmp/both.json",
    ]));
    let Err(refused) = mixed else {
        panic!("a reading and an analysis cannot share a path either");
    };
    assert!(refused.message().contains("/tmp/both.json"), "{refused}");
}

/// The `cli` standard's collection shape, so a caller reads `count` rather than measuring the
/// array, and knows from `has_more` that nothing was left behind.
#[test]
fn every_collection_carries_the_pagination_the_standard_names() {
    let dir = fixture("basic");
    let traced = sva_cli::trace_data(&sva_cli::trace(&dir, "drums").expect("drums traces"));
    for held in ["\"entry\"", "\"down\"", "\"up\""] {
        assert!(
            traced.contains(&format!("{held}: {{ \"items\"")),
            "{held} is a collection: {traced}"
        );
    }
    assert!(
        traced.contains("\"has_more\": false") && traced.contains("\"next_cursor\": null"),
        "answered whole, so the cursor is spent: {traced}"
    );

    let refused = master(&scratch("pagination-refusal"))
        .err()
        .expect("no master");
    let json = error_envelope(refused.code(), &refused.message(), &refused.diagnostics());
    assert!(
        json.contains("\"diagnostics\": { \"items\""),
        "a refusal's findings are a collection too: {json}"
    );
    assert!(
        json.contains(&format!("\"count\": {}", refused.diagnostics().len())),
        "counted once, by the tool: {json}"
    );
}

/// Each reading caps its own series and names its own resume point, so the arithmetic is
/// checked where it is written, not only through the one helper they share.
#[test]
fn a_capped_reading_names_the_second_its_items_stop_before() {
    let rendered = master(&fixture("basic")).expect("basic renders");
    let at = |json: &str| -> f64 {
        json.split("\"next_cursor\": \"")
            .nth(1)
            .and_then(|tail| tail.split('s').next())
            .unwrap_or_else(|| panic!("a capped reading names where to resume: {json}"))
            .parse()
            .expect("a number of seconds")
    };

    let samples = rendered.answer(PROBE, Representation::Samples).unwrap();
    assert_eq!(
        at(&sva_core::value_json(&samples.value, Some(3), false)),
        3.0 / 44_100.0,
        "a plane resumes at the sample after the last one shown"
    );

    for representation in [Representation::Bands, Representation::Loudness] {
        let answer = rendered.answer(PROBE, representation).unwrap();
        let json = sva_core::value_json(&answer.value, Some(3), false);
        let resume = at(&json);
        assert!(
            resume > 0.0 && resume < secs(&rendered),
            "{representation:?} resumes inside its own window, not at {resume}"
        );
    }
}

/// The interval a reading reports is the one the root's support ended.
#[test]
fn a_render_reports_the_interval_its_support_ended() {
    let dir = scratch("support");
    put(&dir, "tone", "crop(sin(2*pi*440*t), 0s, 0.3s)\n");
    let rendered = probe(&dir, "@tone").expect("the tone ends");
    let end = secs(&rendered);
    assert_eq!(end, 0.3);
    assert_eq!(
        plane(&rendered, PROBE).len(),
        (end * 44_100.0).round() as usize
    );
}
