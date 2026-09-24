// Concern: exercises sva-cli's run/probe/answer/write end to end over fixtures | Non-concern: unit-level parsing or collapse math | IO: (fixtures/*) -> asserted Rendered, JSON or CliError

mod helpers;
mod tempo;
mod written;

use helpers::{entry, fixture, json_of, plane, put, scratch, secs};
use sva_ast::Dir;
use sva_cli::{CliError, error_envelope};
use sva_core::{Job, PROBE, ROOT, execute, probe, run};
use sva_engine::{Output, Representation, Source};

#[test]
fn basic_fixture_renders_with_bpm_meter_and_a_repeat_desugared() {
    let rendered = run(&fixture("basic")).expect("basic fixture should render");
    assert_eq!(
        secs(&rendered),
        1.0,
        "no filename span, so the 1.0s default"
    );
    assert!(!plane(&rendered, ROOT).is_empty());

    let root = entry(&rendered, ROOT);
    assert_eq!(root.share, Some(1.0), "a node's share of itself is 1.0");
    assert_eq!(root.depth, 0);
    assert_eq!(root.clipped, Some(false));
    assert!(root.peak > 0.0 && root.peak < 1.0);
}

/// `master` can never carry a filename span, so a multi-section arrangement's duration comes
/// from the furthest `crop` window its own desugared `concat` leaves behind.
#[test]
fn an_arranged_root_takes_its_duration_from_its_own_crop_windows() {
    let rendered = run(&fixture("arranged")).expect("arranged fixture should render");
    assert_eq!(secs(&rendered), 6.0, "1 bar + 2 bars at 120bpm 4/4");
    let samples = plane(&rendered, ROOT);
    assert_eq!(samples.len(), 6 * 44100);
    assert!(
        samples[5 * 44100..].iter().any(|s| *s != 0.0),
        "the last section sounds"
    );
}

/// Naming a node renders that node as the root: its subtree alone, over its own extent. `t`
/// is global, so the samples are the very ones the master would have seen.
#[test]
fn a_named_node_renders_as_its_own_root_over_its_own_extent() {
    let dir = fixture("arranged");
    let at = |until| {
        execute(Job {
            target: Some("drop-2b"),
            until,
            ..Job::over(&Dir::at(&dir))
        })
        .unwrap()
    };
    let whole = run(&dir).unwrap();
    let part = at(None);
    let wider = at(Some(sva_core::WindowEdge::Secs(6.0)));

    assert_eq!(secs(&whole), 6.0);
    assert_eq!(secs(&part), 4.0, "two bars at 120bpm, its own extent");
    assert_eq!(part.target, "drop-2b");
    let (narrow, broad) = (plane(&part, "drop-2b"), plane(&wider, "drop-2b"));
    assert_eq!(narrow.len(), 4 * 44100);
    for (i, (a, b)) in narrow.iter().zip(broad).enumerate() {
        assert!(
            (a - b).abs() < 1e-12,
            "sample {i} moved with the window: {a} against {b}"
        );
    }
}

/// The rate is an observation parameter: the same seconds on a different grid, with nothing
/// in the expression able to read which grid it landed on.
#[test]
fn a_stated_sample_rate_lays_the_same_seconds_on_a_different_grid() {
    let dir = fixture("arranged");
    let at = |hz: Option<u32>| {
        execute(Job {
            target: Some("drop-2b"),
            sample_rate: hz,
            ..Job::over(&Dir::at(&dir))
        })
        .unwrap()
    };
    let base = at(None);
    let fine = at(Some(96_000));
    assert_eq!(base.config.rate, 44_100, "unstated is the default");
    assert_eq!(fine.config.rate, 96_000);
    assert_eq!(secs(&base), secs(&fine), "the same seconds either way");
    assert_eq!(plane(&base, "drop-2b").len(), 4 * 44_100);
    assert_eq!(plane(&fine, "drop-2b").len(), 4 * 96_000);
}

/// FORMAT 16.3: a reading runs against a horizon, and `--from`/`--to` ARE that horizon —
/// past the node's own extent or inside it, the window the caller named is the one it gets.
#[test]
fn an_explicit_window_is_the_horizon_a_reading_runs_against() {
    let dir = fixture("arranged");
    let asked = |from: Option<sva_core::WindowEdge>, until: Option<sva_core::WindowEdge>| {
        execute(Job {
            target: Some("intro-1b"),
            from,
            until,
            ..Job::over(&Dir::at(&dir))
        })
    };
    let secs_at = |v: Option<f64>| v.map(sva_core::WindowEdge::Secs);
    let held = |from, until| {
        let r = asked(secs_at(from), secs_at(until)).unwrap();
        (
            r.config.horizon.start_secs,
            secs(&r),
            plane(&r, "intro-1b").len(),
        )
    };
    assert_eq!(held(None, None), (0.0, 2.0, 2 * 44100), "its own extent");
    assert_eq!(held(None, Some(5.5)), (0.0, 5.5, 242_550), "past it");
    assert_eq!(held(None, Some(0.5)), (0.0, 0.5, 22_050), "inside it");
    assert_eq!(held(Some(0.5), Some(1.5)), (0.5, 1.5, 44_100), "both edges");
    assert!(
        matches!(
            asked(secs_at(Some(1.0)), secs_at(Some(0.5))),
            Err(CliError::Usage(_))
        ),
        "a window that ends before it starts is no window"
    );
}

#[test]
fn no_tempo_fixture_skips_the_bridge_and_still_renders() {
    let rendered = run(&fixture("no-tempo")).expect("no-tempo fixture should render");
    assert_eq!(secs(&rendered), 1.0);
    assert_eq!(entry(&rendered, ROOT).share, Some(1.0));
}

#[test]
fn a_ledger_envelope_names_its_target_window_and_every_node_under_it() {
    let rendered = run(&fixture("basic")).unwrap();
    let json = json_of(&rendered, "ledger", Representation::Ledger { depth: 8 });

    assert!(json.contains("\"status\": \"success\""));
    assert!(json.contains("\"target\": \"master\""));
    assert!(json.contains("\"sample_rate\": 44100"));
    assert!(json.contains("\"start_secs\": 0"));
    assert!(json.contains("\"end_secs\": 1"));
    assert!(json.contains("\"written\": { \"items\": [], \"pagination\""));
    assert!(json.contains("\"node\": \"master\""));
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
    let dir = fixture("basic");
    let rendered = probe(&dir, "sin(2*pi*440*t)").unwrap();

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
    let before = run(&dir).unwrap();
    let probed = probe(&dir, "@drums*0.5").expect("probe should render");

    assert_eq!(secs(&probed), secs(&before));
    let dry = entry(&before, ROOT).rms;
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
    assert!(matches!(probe(&dir, "@nope*2"), Err(CliError::Engine(_))));
    assert!(matches!(
        probe(&dir, "lp(@drums, cutoff="),
        Err(CliError::BadProbe(_))
    ));
}

/// A caller reading this JSON must be able to subtract the floor without a second call, so
/// every band carries its own beside the measurement it belongs to.
#[test]
fn a_bands_rendering_reports_every_bands_own_impulse_floor_beside_it() {
    let rendered = run(&fixture("basic")).unwrap();
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

    let Output::Bands(bands) = rendered.answer(ROOT, Representation::Bands).unwrap().value else {
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

    let err = run(&dir).err().expect("a dangling ref refuses");
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

    let err = run(&dir).err().expect("a dangling ref refuses");
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
    assert!(matches!(run(&dir), Err(CliError::Refusals(_))));
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
    put(&dir, "master", "join(t, t) + join(t, t, t)\n");

    let err = run(&dir).err().expect("a width conflict refuses");
    let [diagnostic] = err.diagnostics().try_into().ok().unwrap();
    assert_eq!(diagnostic.code, "type.width_mismatch");
    assert_eq!(diagnostic.file.as_deref(), Some("master"));
    assert!(diagnostic.message.contains("2 components meet 3"));
}

/// The peak figure is the one thing this rendering cannot measure to standard, so it says so
/// in the report rather than leaving a reader to assume a true-peak reading.
#[test]
fn a_loudness_rendering_names_its_peak_as_a_sample_peak() {
    let rendered = run(&fixture("basic")).unwrap();
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
    let rendered = run(&fixture("basic")).unwrap();
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
    let dir = fixture("basic");
    let measured = |expr: &str| {
        let rendered = probe(&dir, expr).expect("the probe renders");
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
    assert_eq!(clean.rate_dependent, 0, "a function of t alone");
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

/// A sampled loop is a different signal at the oversampled rate, so a render holding one is
/// counted beside the figure rather than read as pure alias.
#[test]
fn a_rate_dependent_instance_is_counted_beside_the_figures() {
    let dir = fixture("basic");
    let count = |expr: &str| {
        let rendered = probe(&dir, expr).expect("the probe renders");
        sva_engine::rate_dependent(&rendered.graph, &rendered.target).unwrap()
    };
    assert!(count("sin(2*pi*440*t)").is_empty());
    assert_eq!(
        count("sample(sin(2*pi*220*t)) + 0.5*self(t - 1sp)").len(),
        1,
        "a sampled loop is a family indexed by the rate"
    );
    assert!(
        count("rand(0, seed=3)").is_empty(),
        "a keyed constant is rate-free"
    );
}

/// `t = 0` is the transient and what a `crop` declares before it is buildup, so a swoosh
/// written at negative `t` reaches a hit placed later without anyone nudging it.
#[test]
fn a_declared_pre_roll_renders_before_zero_and_an_undeclared_one_stays_silent() {
    let dir = scratch("preroll");
    put(&dir, "swoosh", "sin(2*pi*300*t)\n");
    put(&dir, "hit", "crop(@swoosh(t), -0.5s, 0.5s)\n");
    put(&dir, "master", "crop(@hit(t - 1s), 0s, 2s)\n");

    let rendered = run(&dir).expect("the composition renders");
    assert_eq!(rendered.config.horizon.start_secs, -0.5);
    let played = plane(&rendered, ROOT);
    let energy = |from: f64, to: f64| {
        let at = |s: f64| ((s + 0.5) * 44100.0) as usize;
        played[at(from)..at(to)].iter().map(|s| s * s).sum::<f64>()
    };
    assert!(energy(0.6, 0.9) > 1.0, "the buildup arrives before the hit");
    assert!(
        energy(0.1, 0.4) < 1e-9,
        "and nothing reaches further back than declared"
    );

    put(&dir, "hit", "crop(@swoosh(t), 0s, 0.5s)\n");
    let plain = run(&dir).expect("the composition renders");
    assert_eq!(plain.config.horizon.start_secs, 0.0);
    let quiet = plane(&plain, ROOT);
    assert!(
        quiet[..44100].iter().map(|s| s * s).sum::<f64>() < 1e-9,
        "reaching before zero is never inferred"
    );
}

/// A page holds a registry, not a directory. Only what the target reaches is pulled, and the
/// samples are the ones a whole load produces, tempo and all.
#[test]
fn a_reaching_load_pulls_one_targets_closure_and_renders_what_a_whole_load_would() {
    let mut held = sva_ast::Composition::new();
    held.insert("variables/bpm", "120\n");
    held.insert("variables/meter", "4/4\n");
    held.insert("lead-2b", "sin(2*pi*220*t)\n");
    held.insert("master", "@lead-2b*0.5\n");
    let clean = held.clone();
    held.insert("nothing-reaches-this", "this is not an expression (\n");

    let job = |source: &sva_ast::Composition, reaching| {
        execute(Job {
            target: Some("lead-2b"),
            reaching,
            ..Job::over(source)
        })
    };

    let reached = job(&held, true).expect("only what `lead-2b` reaches is read");
    assert_eq!(secs(&reached), 4.0, "two bars at 120bpm, read by name");

    let whole = job(&clean, false).expect("the clean registry renders either way");
    assert_eq!(
        plane(&reached, "lead-2b"),
        plane(&whole, "lead-2b"),
        "the same samples, whatever was loaded around them"
    );
    assert!(
        matches!(job(&held, false), Err(CliError::Refusals(_))),
        "a whole load reads the broken node, and refuses on it"
    );
}

/// A closed form answers `lines` off its own spectral sum, and no buffer is allocated behind it.
#[test]
fn a_law_answers_lines_with_no_buffer_behind_it() {
    let dir = fixture("basic");
    let rendered = execute(Job {
        target: Some("sin(2*pi*261.63*t) + sin(2*pi*329.63*t)"),
        representations: vec![Representation::Lines],
        ..Job::over(&Dir::at(&dir))
    })
    .unwrap();
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
        let rendered = execute(Job {
            target: Some(expr),
            representations: vec![Representation::Lines],
            ..Job::over(&Dir::at(&dir))
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

/// FORMAT 14: one render, many readings. Two readings off one buffer collapse it once, and
/// the cache report names that one lookup rather than one per `--as`.
#[test]
fn two_readings_share_one_collapse() {
    let dir = scratch("two-readings");
    put(
        &dir,
        "master",
        "; Models: a stack | Neglects: an envelope | IO: (t) -> amplitude | Tags: test\n\
         sin(2*pi*100*t) + sin(2*pi*200*t)\n",
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sva-cli"))
        .current_dir(&dir)
        .env("SVA_CACHE", scratch("two-readings-cache"))
        .args(["render", "master", "--as", "loudness", "--as", "envelope"])
        .output()
        .expect("the binary runs");
    let printed = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{printed}");
    assert!(printed.contains("\"loudness\""), "{printed}");
    assert!(printed.contains("\"envelope\""), "{printed}");

    let lookups = printed
        .split("\"lookups\": ")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("the cache report lists every lookup");
    assert_eq!(
        lookups.matches("\"kind\": \"samples\"").count(),
        1,
        "two readings collapse the one node once: {lookups}"
    );
}

/// `trace` names one instance of a parameterized file `<path>(<name>=<value>)`. `render`
/// reads the same name for the same node, rather than parsing it as argv arithmetic.
#[test]
fn render_accepts_the_instance_name_trace_prints() {
    let dir = scratch("instance-name");
    put(
        &dir,
        "shepard/step",
        "; Models: one step | Neglects: the stepping | IO: (t, s) -> amplitude | Tags: test\n\
         sin(2*pi*55*pow(2, s/12)*t)\n",
    );
    put(
        &dir,
        "master",
        "; Models: two steps | Neglects: a glide | IO: (t) -> amplitude | Tags: test\n\
         @shepard/step(t, s=0) + @shepard/step(t, s=12)\n",
    );
    let read = |name: &str| {
        let rendered = execute(Job {
            target: Some(name),
            representations: vec![Representation::Lines],
            reaching: true,
            ..Job::over(&Dir::at(&dir))
        })
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let node = rendered.target.clone();
        let Output::Lines(lines) = rendered
            .answer(&node, Representation::Lines)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .value
        else {
            panic!("{name}: expected lines");
        };
        let mut hz: Vec<i64> = lines.iter().map(|l| l.hz.round() as i64).collect();
        hz.sort_unstable();
        hz
    };
    assert_eq!(read("shepard/step(s=0)"), vec![-55, 55]);
    assert_eq!(read("shepard/step(s=12)"), vec![-110, 110]);
    assert_eq!(
        read("@shepard/step(t, s=0)"),
        read("shepard/step(s=0)"),
        "the instance name and the ref that makes it are one node"
    );
    assert_eq!(
        read("shepard/step(s=max(0, 12))"),
        vec![-110, 110],
        "a bind whose own value is a call keeps its commas"
    );
}

/// FORMAT 5.1's own literals reach argv: `end` is the node's own extent, `<n>s` seconds and
/// `<n>b` bars, so the spelling `crop(x, 0s, 128b)` uses names the same window here.
#[test]
fn to_end_and_to_bars_render_the_whole_piece() {
    let dir = scratch("window-units");
    let head = "; Models: a probe | Neglects: everything | IO: (t) -> amplitude | Tags: probe\n";
    std::fs::create_dir_all(dir.join("variables")).expect("a variables directory");
    put(&dir, "variables/bpm", &format!("{head}120\n"));
    put(&dir, "variables/meter", &format!("{head}4/4\n"));
    put(
        &dir,
        "master",
        &format!("{head}crop(sin(2*pi*440*t), 0s, 2b)\n"),
    );

    let window = |flag: &str, raw: &str| {
        let argv: Vec<String> = ["render", "--as", "samples", flag, raw]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let Ok(sva_cli::Command::Render(args)) = sva_cli::parse_args(&argv) else {
            panic!("`{flag} {raw}` should parse");
        };
        let held = execute(Job {
            from: args.from,
            until: args.to,
            ..Job::over(&Dir::at(&dir))
        })
        .unwrap_or_else(|e| panic!("`{flag} {raw}`: {}", e.message()))
        .config
        .horizon;
        (held.start_secs, held.end_secs)
    };

    let whole = window("--to", "end");
    assert_eq!(whole, (0.0, 4.0), "two bars at 120 bpm is four seconds");
    assert_eq!(window("--to", "4s"), whole, "`4s` is the same four seconds");
    assert_eq!(window("--to", "2b"), whole, "`2b` reads the same tempo");
    assert_eq!(window("--to", "4"), whole, "a bare number is still seconds");

    assert_eq!(
        window("--from", "end"),
        whole,
        "`--from end` is the extent naming no `--from` gives"
    );
    assert_eq!(window("--from", "1b"), (2.0, 4.0), "one bar in");
    assert_eq!(window("--from", "2s"), (2.0, 4.0), "the same two seconds");
}

/// A render pulls only the closure its target reaches, so a file its call sites parameterize
/// answers for no instance on its own. It names the ones that exist instead — the list
/// `trace` prints, each a target a reading may then take.
#[test]
fn render_of_a_parameterized_file_lists_its_instances() {
    let dir = Dir::at(fixture("reverb"));
    let refused = execute(Job {
        target: Some("fx/comb"),
        reaching: true,
        representations: vec![Representation::Lines],
        ..Job::over(&dir)
    });
    let Err(CliError::Engine(e)) = refused else {
        panic!("a file with eight instances answers for none of them");
    };
    assert_eq!(e.code(), "engine.ambiguous_node");
    let message = e.to_string();
    assert!(message.contains("8 instances"), "{message}");
    assert!(message.contains("fx/comb(delay="), "{message}");
}

/// One instance is no list: a file exactly one call site parameterizes is that instance, the
/// same resolution `trace` and a reading by name make.
#[test]
fn render_of_a_file_one_call_site_parameterizes_renders_that_instance() {
    let dir = scratch("sole-instance");
    put(
        &dir,
        "fx/comb",
        "; Models: a comb | Neglects: nothing | IO: (t, d) -> amplitude | Tags: fx\n0.5*sin(2*pi*220*t) + 0.5*self(t - d*1s)\n",
    );
    put(
        &dir,
        ROOT,
        "; Models: a mix | Neglects: nothing | IO: (t) -> amplitude | Tags: mix\n@fx/comb(t, d=0.01)\n",
    );
    let rendered = execute(Job {
        target: Some("fx/comb"),
        reaching: true,
        representations: vec![Representation::Lines],
        ..Job::over(&Dir::at(&dir))
    })
    .expect("the one instance its call site named");
    let answer = rendered
        .answer(&rendered.target, Representation::Lines)
        .expect("the lines of that instance");
    let Output::Lines(lines) = answer.value else {
        panic!("expected a line list");
    };
    assert!(!lines.is_empty(), "the comb rings on its own line");
}

/// Asking for a node the graph does not hold is a missing resource, not a malformed request.
#[test]
fn a_missing_node_is_not_found() {
    let dir = scratch("absent-node");
    put(&dir, "master", "sin(2*pi*300*t)\n");

    let rendered = run(&dir).expect("the composition renders");
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
    let rendered = run(&dir).expect("basic renders");
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

/// One path, one reading. Two `--as` at the same destination left one of them on disk while
/// the envelope named both as written, so the response said what the filesystem did not.
#[test]
fn two_readings_at_one_destination_refuse_before_anything_is_rendered() {
    let argv = |parts: &[&str]| -> Vec<String> { parts.iter().map(|s| (*s).to_string()).collect() };
    let shared = sva_cli::parse_args(&argv(&[
        "render",
        "--as",
        "lines=/tmp/one.json",
        "--as",
        "atoms=/tmp/one.json",
    ]));
    let Err(refused) = shared else {
        panic!("one path cannot hold two readings");
    };
    assert_eq!(refused.code(), "validation_error");
    assert!(refused.message().contains("/tmp/one.json"), "{refused}");

    assert!(
        sva_cli::parse_args(&argv(&[
            "render",
            "--as",
            "lines=/tmp/one.json",
            "--as",
            "atoms=/tmp/two.json",
        ]))
        .is_ok(),
        "two paths are two readings"
    );
    assert!(
        sva_cli::parse_args(&argv(&["render", "--as", "lines", "--as", "atoms"])).is_ok(),
        "and stdout holds both under their own keys"
    );
    assert!(
        sva_cli::parse_args(&argv(&[
            "render",
            "--as",
            "lines=one.json",
            "--as",
            "atoms=./one.json",
        ]))
        .is_err(),
        "`./one.json` and `one.json` are one file"
    );

    // `analyze` builds its destinations from two lists: the representations and the analyses.
    let mixed = sva_cli::parse_args(&argv(&[
        "analyze",
        "in.wav",
        "--as",
        "spectrum=/tmp/both.json",
        "--as",
        "onsets=/tmp/both.json",
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

    let refused = run(&scratch("pagination-refusal"))
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
    let rendered = run(&fixture("basic")).expect("basic renders");
    let at = |json: &str| -> f64 {
        json.split("\"next_cursor\": \"")
            .nth(1)
            .and_then(|tail| tail.split('s').next())
            .unwrap_or_else(|| panic!("a capped reading names where to resume: {json}"))
            .parse()
            .expect("a number of seconds")
    };

    let samples = rendered.answer(ROOT, Representation::Samples).unwrap();
    assert_eq!(
        at(&sva_core::value_json(&samples.value, Some(3), false)),
        3.0 / 44_100.0,
        "a plane resumes at the sample after the last one shown"
    );

    for representation in [Representation::Bands, Representation::Loudness] {
        let answer = rendered.answer(ROOT, representation).unwrap();
        let json = sva_core::value_json(&answer.value, Some(3), false);
        let resume = at(&json);
        assert!(
            resume > 0.0 && resume < secs(&rendered),
            "{representation:?} resumes inside its own window, not at {resume}"
        );
    }
}
