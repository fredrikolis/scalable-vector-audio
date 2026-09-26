// Concern: the readings that leave the object, as audio or as a file | Non-concern: what a render answers in the envelope (the parent suite) | IO: (fixtures/*) -> a written file

use std::path::{Path, PathBuf};

use crate::helpers::{buffer, fixture, json_of, ledger, put, scratch};
use sva_ast::Dir;
use sva_cli::{SampleEncoding, write_channels, write_wav};
use sva_core::{Job, ROOT, execute, run};
use sva_engine::{Output, Representation};

#[test]
fn a_samples_destination_writes_float_audio_matching_the_rendered_samples() {
    let rendered = run(&fixture("basic")).unwrap();
    let dir = scratch("wav");
    let path = dir.join("out.wav");
    let pcm: Vec<f32> = buffer(&rendered, ROOT).as_f32(0);
    write_wav(&pcm, rendered.config.rate, &path, SampleEncoding::Float).unwrap();

    let mut reader = hound::WavReader::open(&path).unwrap();
    assert_eq!(reader.spec().channels, 1);
    assert_eq!(reader.spec().bits_per_sample, 32);
    assert_eq!(reader.spec().sample_format, hound::SampleFormat::Float);
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples, pcm, "no quantisation between render and file");
}

/// A mono report is untouched, and `channel` appearing is what tells a reader a node carries
/// components at all.
#[test]
fn a_stereo_composition_reports_a_channel_per_component_and_a_mono_one_reports_none() {
    let rendered = run(&fixture("stereo")).unwrap();
    let entries = ledger(&rendered, ROOT);
    let master: Vec<_> = entries.iter().filter(|e| e.node == ROOT).collect();
    assert_eq!(master.len(), 2, "one entry per component");
    assert_eq!(
        (master[0].channel, master[1].channel),
        (Some(0), Some(1)),
        "each names which component it measured"
    );
    assert!(master[0].rms > master[1].rms, "p=0.3 sits left of centre");

    let json = json_of(&rendered, "ledger", Representation::Ledger { depth: 8 });
    assert!(json.contains("\"node\": \"master\", \"channel\": 0,"));
}

#[test]
fn a_stereo_render_writes_an_interleaved_two_channel_wav() {
    let rendered = run(&fixture("stereo")).unwrap();
    let dir = scratch("stereo-wav");
    let path = dir.join("out.wav");
    let held = buffer(&rendered, ROOT);
    let owned: Vec<Vec<f32>> = (0..held.width).map(|c| held.as_f32(c)).collect();
    let planes: Vec<&[f32]> = owned.iter().map(Vec::as_slice).collect();
    write_channels(&planes, rendered.config.rate, &path, SampleEncoding::Float).unwrap();

    let mut reader = hound::WavReader::open(&path).unwrap();
    assert_eq!(reader.spec().channels, 2);
    let pcm: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(pcm.len(), held.len() * 2);
    let loudest = pcm
        .chunks_exact(2)
        .max_by(|a, b| a[0].abs().total_cmp(&b[0].abs()))
        .unwrap();
    assert!(
        loudest[0].abs() > loudest[1].abs(),
        "the left component leads in every frame, so interleaving kept the order"
    );
}

/// Each bounce lands on the other side, one `ch` crossing apart.
#[test]
fn the_ping_pong_fixture_bounces_side_to_side_in_the_stereo_representation() {
    let rendered = execute(Job {
        target: Some("pingpong"),
        ..Job::over(&Dir::at(fixture("stereo")))
    })
    .unwrap();
    let Output::Stereo(image) = rendered
        .answer("pingpong", Representation::Stereo { frame_secs: 0.05 })
        .unwrap()
        .value
    else {
        panic!("expected a stereo image");
    };
    assert_eq!(image.channels, 2);

    let bounces: Vec<(f64, f64)> = image
        .frames
        .iter()
        .filter(|f| f.mid_rms > 0.001)
        .map(|f| (f.t_secs, f.balance_db))
        .collect();
    assert!(bounces.len() >= 5, "at least five bounces: {bounces:?}");
    for (n, (t, balance)) in bounces.iter().enumerate() {
        assert!(
            (t - 0.25 * n as f64).abs() < 1e-9,
            "bounce {n} lands a quarter second after the last: {t}"
        );
        let side = if n % 2 == 0 { -1.0 } else { 1.0 };
        assert_eq!(*balance, side * 160.0, "bounce {n} must be hard panned");
    }
}

/// The destination is the one place a reading leaves the object it would have printed in.
#[test]
fn a_json_destination_carries_every_sample_where_stdout_caps_them() {
    let dir = scratch("destination");
    let out = scratch("destination-out");
    put(&dir, "master", "crop(sin(2*pi*220*t), 0s, 0.2s)\n");
    let rendered = run(&dir).unwrap();
    let answer = rendered.answer(ROOT, Representation::Samples).unwrap();
    let path: PathBuf = out.join("s.json");
    sva_cli::write_destination(
        "samples",
        &answer,
        &path,
        &sva_cli::Framing {
            target: ROOT.to_string(),
            rate: rendered.config.rate,
            horizon: rendered.config.horizon,
            profile: rendered.config.profile.name,
            encoding: SampleEncoding::Float,
            skim: false,
            replace: true,
        },
    )
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"count\": 8820"));
    assert!(text.contains("\"has_more\": false"));

    let inside = dir.join("out.wav");
    assert!(
        sva_cli::refuse_inside(&dir, &inside).is_err(),
        "a destination inside the composition would be read as a node"
    );
    assert!(sva_cli::refuse_inside(&dir, Path::new("/tmp/out.wav")).is_ok());
}

/// A reading that refuses must not leave an earlier reading's file behind: every answer is
/// taken before any destination is opened.
#[test]
fn a_later_reading_refusing_writes_no_earlier_destination() {
    let dir = scratch("partial-write");
    let out = scratch("partial-write-out");
    put(&dir, "master", "crop(sin(2*pi*220*t), 0s, 0.05s)\n");
    let path = out.join("out.wav");

    let run = std::process::Command::new(env!("CARGO_BIN_EXE_sva-cli"))
        .current_dir(&dir)
        .args([
            "render",
            "--as",
            &format!("samples={}", path.display()),
            "--as",
            "stereo",
        ])
        .output()
        .expect("the binary runs");
    let printed = String::from_utf8_lossy(&run.stdout);
    assert!(printed.contains("\"status\": \"error\""), "{printed}");
    assert!(printed.contains("type.width_mismatch"), "{printed}");
    assert!(
        !path.exists(),
        "a reading that refused left {} behind",
        path.display()
    );
}

/// A library caller passed no argv, and must read back the code argv would have given.
#[test]
fn a_non_audio_reading_at_a_wav_path_refuses_the_same_way_on_both_sides() {
    let argv: Vec<String> = ["render", "--as", "lines=/tmp/out.wav"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let Err(on_argv) = sva_cli::parse_args(&argv) else {
        panic!("a `.wav` path carries audio, and `lines` is not audio");
    };

    let rendered = sva_core::probe(&fixture("basic"), "sin(2*pi*440*t)").expect("a probe");
    let answer = rendered
        .answer(sva_core::PROBE, Representation::Lines)
        .expect("lines answers");
    let Err(as_a_library) = sva_cli::write_destination(
        "lines",
        &answer,
        Path::new("/tmp/out.wav"),
        &sva_cli::Framing {
            target: ROOT.to_string(),
            rate: rendered.config.rate,
            horizon: rendered.config.horizon,
            profile: rendered.config.profile.name,
            encoding: SampleEncoding::Float,
            skim: false,
            replace: true,
        },
    ) else {
        panic!("and the library surface refuses it too");
    };
    assert_eq!(on_argv.code(), as_a_library.code());
    assert_eq!(on_argv.exit_code(), as_a_library.exit_code());
}

/// A path already taken is the one a retry repeats: until the caller says so, the file on
/// disk is the one that was there, and no other destination is opened either.
#[test]
fn a_destination_that_already_holds_a_file_refuses_until_the_caller_confirms() {
    let dir = scratch("replace");
    let out = scratch("replace-out");
    put(&dir, "master", "sin(2*pi*220*t)\n");
    let taken = out.join("taken.json");
    let fresh = out.join("fresh.json");
    std::fs::write(&taken, "the file that was there\n").unwrap();

    let render = |extra: &[&str]| {
        let mut args = vec![
            "render".to_string(),
            "--as".to_string(),
            format!("lines={}", fresh.display()),
            "--as".to_string(),
            format!("atoms={}", taken.display()),
        ];
        args.extend(extra.iter().map(|a| (*a).to_string()));
        std::process::Command::new(env!("CARGO_BIN_EXE_sva-cli"))
            .current_dir(&dir)
            .args(&args)
            .output()
            .expect("the binary runs")
    };

    let refused = render(&[]);
    let printed = String::from_utf8_lossy(&refused.stdout);
    assert!(printed.contains("\"code\": \"conflict\""), "{printed}");
    assert_eq!(refused.status.code(), Some(4), "{printed}");
    assert_eq!(
        std::fs::read_to_string(&taken).unwrap(),
        "the file that was there\n",
        "nothing replaced it"
    );
    assert!(
        !fresh.exists(),
        "the destination before it was never opened either"
    );

    let confirmed = render(&["--confirm"]);
    let printed = String::from_utf8_lossy(&confirmed.stdout);
    assert!(printed.contains("\"status\": \"success\""), "{printed}");
    assert!(fresh.exists() && taken.exists(), "both were written");
    assert_ne!(
        std::fs::read_to_string(&taken).unwrap(),
        "the file that was there\n",
        "the caller said so"
    );
}
