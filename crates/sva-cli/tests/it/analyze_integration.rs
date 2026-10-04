// Concern: exercises `analyze` over a written WAV, the one reading path with no graph behind it | Non-concern: rendering one (render_integration.rs) | IO: (a .wav) -> an Answer or CliError

use crate::helpers::scratch;

use sva_cli::{SampleEncoding, parse_args, write_channels};
use sva_engine::{Buffer, Output, PSYCHOACOUSTIC_V1, Representation, Source, answer_buffer};

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_string()).collect()
}

fn tone(rate: u32, hz: f64, secs: f64) -> Vec<f32> {
    let len = (f64::from(rate) * secs) as usize;
    (0..len)
        .map(|i| {
            let t = i as f64 / f64::from(rate);
            (std::f64::consts::TAU * hz * t).sin() as f32 * 0.5
        })
        .collect()
}

fn written(name: &str, rate: u32, planes: &[&[f32]]) -> std::path::PathBuf {
    let dir = scratch(name);
    let path = dir.join("in.wav");
    write_channels(planes, rate, &path, SampleEncoding::Float).expect("a written wav");
    path
}

/// The `t_secs` of every item in the answer's own `onsets` collection, in order.
fn onset_times(json: &str) -> Vec<f64> {
    let from = json
        .find("\"onsets\": { \"items\": [")
        .expect("an onsets collection");
    let listed = &json[from..];
    let to = listed.find(']').expect("a closed array");
    listed[..to]
        .split("\"t_secs\": ")
        .skip(1)
        .map(|tail| {
            tail.split([',', ' ', '}'])
                .next()
                .expect("a number")
                .parse()
                .expect("a number")
        })
        .collect()
}

fn decoded(path: &std::path::Path) -> Buffer {
    let (planes, rate) = sva_cli::read_channels(path).expect("a readable wav");
    Buffer::of_planes(
        rate,
        planes
            .iter()
            .map(|p| p.iter().map(|v| f64::from(*v)).collect())
            .collect(),
    )
}

/// The file's own rate stands: nothing resamples, and the reading says which rate it ran at.
#[test]
fn an_analysis_reads_the_files_own_rate_and_never_resamples_it() {
    for rate in [22_050u32, 48_000] {
        let path = written("analyze-rate", rate, &[&tone(rate, 440.0, 0.5)]);
        let buffer = decoded(&path);
        assert_eq!(buffer.rate, rate);
        let answer = answer_buffer(
            "in.wav",
            &buffer,
            Representation::Spectrum {
                max_peaks: 4,
                frame_secs: None,
            },
            PSYCHOACOUSTIC_V1.name,
        )
        .expect("a spectrum off a file");
        assert_eq!(answer.rate, Some(rate));
        assert_eq!(answer.source, Source::Measured);
        assert_eq!(answer.profile, "psychoacoustic-v1");
        let Output::Spectrum(spectrum) = answer.value else {
            panic!("expected a spectrum")
        };
        let peak = spectrum.peaks.first().expect("one peak at least");
        assert!(
            (peak.hz - 440.0).abs() < 20.0,
            "{rate} Hz put the 440 Hz tone at {}",
            peak.hz
        );
    }
}

/// A reading that needs the tree a node was built from has no answer off a file, and says
/// so rather than answering something else.
#[test]
fn a_reading_that_needs_a_graph_refuses_off_a_file() {
    let path = written("analyze-refuse", 8_000, &[&tone(8_000, 220.0, 0.1)]);
    let buffer = decoded(&path);
    for representation in [
        Representation::Ledger { depth: 3 },
        Representation::Alias { oversample: 4 },
        Representation::Lines,
    ] {
        let err = answer_buffer("in.wav", &buffer, representation, PSYCHOACOUSTIC_V1.name)
            .err()
            .unwrap_or_else(|| panic!("{} must refuse", representation.name()));
        assert_eq!(err.code(), "engine.observation_needs_a_graph");
    }
}

/// Argv says which readings a file answers at all, before a byte of it is read.
#[test]
fn analyze_takes_only_the_readings_a_buffer_answers_and_needs_a_wav() {
    for name in sva_cli::ANALYZE_REPRESENTATIONS {
        assert!(
            parse_args(&argv(&["analyze", "/tmp/a.wav", "--representation", name])).is_ok(),
            "`{name}` is one a buffer answers"
        );
    }
    for name in ["ledger", "alias", "bindings", "lines", "atoms"] {
        assert!(
            parse_args(&argv(&["analyze", "/tmp/a.wav", "--representation", name])).is_err(),
            "`{name}` needs a rendered graph"
        );
    }
    assert!(
        parse_args(&argv(&[
            "analyze",
            "/tmp/a.flac",
            "--representation",
            "bands"
        ]))
        .is_err()
    );
}

/// Two components in, two components read: an interleaved file decodes back to the planes
/// it was written from.
#[test]
fn a_two_channel_file_answers_a_stereo_image_a_mono_one_cannot() {
    let rate = 16_000u32;
    let left = tone(rate, 300.0, 0.4);
    let right: Vec<f32> = left.iter().map(|s| s * 0.25).collect();
    let path = written("analyze-stereo", rate, &[&left, &right]);
    let buffer = decoded(&path);
    assert_eq!(buffer.width(), 2);

    let Output::Stereo(image) = answer_buffer(
        "in.wav",
        &buffer,
        Representation::Stereo { frame_secs: 0.05 },
        PSYCHOACOUSTIC_V1.name,
    )
    .expect("a stereo image")
    .value
    else {
        panic!("expected a stereo image")
    };
    assert_eq!(image.channels, 2);
    assert!(image.overall.balance_db < 0.0, "the left side leads");

    let mono = written("analyze-mono", rate, &[&left]);
    let err = answer_buffer(
        "in.wav",
        &decoded(&mono),
        Representation::Stereo { frame_secs: 0.05 },
        PSYCHOACOUSTIC_V1.name,
    )
    .expect_err("one component is no image");
    assert_eq!(err.code(), "type.width_mismatch");
}

/// A frame the transform cannot hold refuses on both paths rather than shrinking silently
/// or overflowing the size it was asked for.
#[test]
fn a_frame_past_the_transform_bound_refuses_on_both_paths() {
    for span in ["1e9", "1e300"] {
        let refused = std::process::Command::new(env!("CARGO_BIN_EXE_sva-cli"))
            .current_dir(scratch("frame-bound"))
            .args([
                "render",
                "@master",
                "--representation",
                &format!("spectrum(frame={span})"),
            ])
            .output()
            .expect("the binary runs");
        let printed = String::from_utf8_lossy(&refused.stdout);
        assert_eq!(refused.status.code(), Some(3), "{printed}");
        assert!(printed.contains("past the"), "{printed}");
    }

    let rate = 8_000u32;
    let path = written("frame-bound-file", rate, &[&tone(rate, 220.0, 0.1)]);
    let refused = std::process::Command::new(env!("CARGO_BIN_EXE_sva-cli"))
        .args([
            "analyze",
            &path.display().to_string(),
            "--representation",
            "spectrum(frame=1e9)",
        ])
        .output()
        .expect("the binary runs");
    let printed = String::from_utf8_lossy(&refused.stdout);
    assert_eq!(refused.status.code(), Some(3), "{printed}");
    assert!(printed.contains("past the"), "{printed}");
}

/// BRIEF section 9 gates a groove on onsets to 1 ms. A frame says which transient, never
/// when: the flux names the frame and the samples inside it say where the strike is.
#[test]
fn onsets_of_a_grid_land_within_one_millisecond() {
    let rate = 48_000u32;
    let struck = [0.25f64, 0.5, 0.75];
    let len = (f64::from(rate) * 1.0) as usize;
    let mut samples = vec![0.0f32; len];
    for at in struck {
        let from = (at * f64::from(rate)) as usize;
        for n in 0..(f64::from(rate) * 0.08) as usize {
            let t = n as f64 / f64::from(rate);
            let shape = (1.0 - (-t / 0.0015).exp()) * (-t / 0.02).exp();
            let wave = (std::f64::consts::TAU * 440.0 * t).sin();
            if let Some(s) = samples.get_mut(from + n) {
                *s += (0.7 * shape * wave) as f32;
            }
        }
    }
    let path = written("onset-grid", rate, &[&samples]);

    let parsed = parse_args(&argv(&[
        "analyze",
        &path.display().to_string(),
        "--representation",
        "onsets",
    ]))
    .expect("an onset request");
    let sva_cli::Command::Analyze(args) = parsed else {
        panic!("expected an analyze command");
    };
    let json = sva_cli::analyze(&args).expect("an onset reading");
    let found = onset_times(&json);
    assert_eq!(
        found.len(),
        struck.len(),
        "four strikes, four onsets: {found:?}"
    );
    for (heard, wanted) in found.iter().zip(struck) {
        assert!(
            (heard - wanted).abs() <= 0.001,
            "{heard} is more than a millisecond from {wanted}"
        );
    }
    let resolution: f64 = json
        .split("\"resolution_secs\": ")
        .nth(1)
        .and_then(|tail| tail.split([',', ' ', '}']).next())
        .and_then(|n| n.parse().ok())
        .expect("a resolution");
    assert!(
        resolution == 0.0115,
        "a detector states the hop its flux was read at, not the sample period: {json}"
    );
    assert!(
        json.contains("\"source\": \"measured\""),
        "a detector's answer is measured, whatever its resolution"
    );

    let dest = path.with_file_name("onsets.json");
    let parsed = parse_args(&argv(&[
        "analyze",
        &path.display().to_string(),
        "--representation",
        &format!("onsets={}", dest.display()),
    ]))
    .expect("an onset request");
    let sva_cli::Command::Analyze(args) = parsed else {
        panic!("expected an analyze command");
    };
    let json = sva_cli::analyze(&args).expect("an onset reading");
    assert!(
        json.contains(&format!("\"path\": \"{}\"", dest.display())),
        "a destination is named in `written`: {json}"
    );
    assert!(
        onset_times(&std::fs::read_to_string(&dest).expect("a written reading")).len()
            == struck.len(),
        "the reading went to the file the caller named"
    );
}

fn onsets_of(path: &std::path::Path) -> Vec<f64> {
    let parsed = parse_args(&argv(&[
        "analyze",
        &path.display().to_string(),
        "--representation",
        "onsets",
    ]))
    .expect("an onset request");
    let sva_cli::Command::Analyze(args) = parsed else {
        panic!("expected an analyze command");
    };
    onset_times(&sva_cli::analyze(&args).expect("an onset reading"))
}

/// A flux detector needs a frame before the one it scores, and the buffer's first frame has
/// none. A buffer that opens at silence in its first sample and sounds in its first frame
/// saw the strike that started it, so a silent frame stands before the start.
#[test]
fn a_strike_at_zero_is_an_onset() {
    let rate = 48_000u32;
    let struck = [0.0f64, 0.25, 0.5];
    let mut samples = vec![0.0f32; (f64::from(rate) * 0.8) as usize];
    for at in struck {
        let from = (at * f64::from(rate)) as usize;
        for n in 0..(f64::from(rate) * 0.08) as usize {
            let t = n as f64 / f64::from(rate);
            let shape = (1.0 - (-t / 0.0015).exp()) * (-t / 0.02).exp();
            let wave = (std::f64::consts::TAU * 440.0 * t).sin();
            if let Some(s) = samples.get_mut(from + n) {
                *s += (0.7 * shape * wave) as f32;
            }
        }
    }
    let found = onsets_of(&written("onset-at-zero", rate, &[&samples]));
    assert_eq!(found.len(), struck.len(), "a strike at zero too: {found:?}");
    for (heard, wanted) in found.iter().zip(struck) {
        assert!(
            (heard - wanted).abs() <= 0.001,
            "{heard} is more than a millisecond from {wanted}"
        );
    }
}

/// A window's own leakage moves the magnitudes between frames of a steady tone, and a rise
/// that small is no strike. The tone swells in over a quarter second so the buffer opens
/// below the floor: a buffer that opens ON sound reports its own edge, because no reading can
/// see what came before it, and that edge is what `a_strike_at_zero_is_an_onset` asserts.
#[test]
fn a_sustained_tone_has_no_onsets() {
    let rate = 48_000u32;
    let held: Vec<f32> = (0..(f64::from(rate) * 2.0) as usize)
        .map(|i| {
            let t = i as f64 / f64::from(rate);
            let swell = (t / 0.25).min(1.0);
            (swell * (std::f64::consts::TAU * 440.0 * t).sin() * 0.5) as f32
        })
        .collect();
    let found = onsets_of(&written("onset-sustained", rate, &[&held]));
    assert!(found.is_empty(), "a steady tone strikes nothing: {found:?}");
}

/// The same tone one quarter turn along: nothing about an onset may turn on which phase the
/// buffer's first sample happens to hold.
#[test]
fn a_sustained_tones_opening_phase_changes_no_onset() {
    let rate = 48_000u32;
    let at_phase = |quarter: f64| {
        let held: Vec<f32> = (0..(f64::from(rate) * 2.0) as usize)
            .map(|i| {
                let t = i as f64 / f64::from(rate);
                let swell = (t / 0.25).min(1.0);
                (swell * (std::f64::consts::TAU * 440.0 * t + quarter).sin() * 0.5) as f32
            })
            .collect();
        onsets_of(&written("onset-phase", rate, &[&held]))
    };
    assert_eq!(at_phase(0.0), at_phase(std::f64::consts::FRAC_PI_2));
}

/// Strikes a hair past the minimum gap are still separate transients: nothing about one may
/// fold into its neighbour.
#[test]
fn strikes_close_to_the_minimum_gap_are_each_their_own_onset() {
    let rate = 44_100u32;
    let gap = 0.08f64;
    let struck: Vec<f64> = (0..12).map(|k| f64::from(k) * gap).collect();
    let mut samples = vec![0.0f32; (f64::from(rate) * (gap * 12.0 + 0.2)) as usize];
    for at in &struck {
        let from = (at * f64::from(rate)) as usize;
        for n in 0..(f64::from(rate) * 0.06) as usize {
            let t = n as f64 / f64::from(rate);
            let shape = (1.0 - (-t / 0.0015).exp()) * (-t / 0.012).exp();
            let wave = (std::f64::consts::TAU * 440.0 * t).sin();
            if let Some(s) = samples.get_mut(from + n) {
                *s += (0.7 * shape * wave) as f32;
            }
        }
    }
    let found = onsets_of(&written("onset-dense", rate, &[&samples]));
    assert_eq!(found.len(), struck.len(), "every strike: {found:?}");
}

/// A strike is where the sound jumps, whatever it does next or stood on: one whose loudest
/// swing lands 16 ms in, one landing on a swell that climbed to 9 dB under it, and one the
/// buffer ends 5 ms after are each an onset, where it struck.
#[test]
fn a_strike_is_an_onset_after_a_swell_before_its_crest_and_at_the_end() {
    let rate = 48_000u32;
    let sr = f64::from(rate);
    let mut samples = vec![0.0f32; (sr * 1.005) as usize];
    let mut burst = |at: f64, secs: f64, gain: f64| {
        for n in 0..(sr * secs) as usize {
            let t = n as f64 / sr;
            let shape = (1.0 - (-t / 0.0005).exp()) * (-t / 0.02).exp();
            if let Some(s) = samples.get_mut((at * sr) as usize + n) {
                *s += (gain * shape * (std::f64::consts::TAU * 440.0 * t).sin()) as f32;
            }
        }
    };
    burst(0.25, 0.08, 0.3);
    burst(0.266, 0.08, 0.7);
    burst(0.6, 0.08, 0.7);
    burst(1.0, 0.005, 0.7);
    let swell = (0.5 * sr) as usize..(0.5985 * sr) as usize;
    for (n, s) in samples
        .iter_mut()
        .enumerate()
        .take(swell.end)
        .skip(swell.start)
    {
        let t = n as f64 / sr;
        let level = 0.25 * (-(0.5985 - t) / 0.03).exp();
        *s += (level * (std::f64::consts::TAU * 1500.0 * t).sin()) as f32;
    }
    let found = onsets_of(&written("onset-swell-end", rate, &[&samples]));
    let struck = [0.25, 0.6, 1.0];
    assert_eq!(found.len(), struck.len(), "every strike: {found:?}");
    for (heard, wanted) in found.iter().zip(struck) {
        assert!(
            (heard - wanted).abs() <= 0.001,
            "{heard} is more than a millisecond from {wanted}"
        );
    }
}

/// A missing file is the caller's typo, exit 24, never `internal_error`.
#[test]
fn a_missing_wav_is_not_found() {
    let absent = scratch("absent-wav").join("never-written.wav");
    let Err(refused) = sva_cli::read_channels(&absent) else {
        panic!("a wav that was never written cannot be read");
    };
    assert_eq!(refused.code(), "not_found");
    assert_eq!(refused.exit_code(), 24);
}
