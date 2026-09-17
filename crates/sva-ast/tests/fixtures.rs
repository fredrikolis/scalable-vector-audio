// Concern: exercises sva-ast's public parse_composition surface over fixture directories | Non-concern: unit-level module behavior (covered in src/*.rs) | IO: (fixtures/*) -> asserted Graph or Refusal

use std::path::Path;
use sva_ast::{Arg, DiagCode, Expr, Skip, Skipped, Source, parse_composition, parse_expr};

fn tmp(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sva-ast-fixtures-{name}-{:x}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, content: &str) {
    std::fs::write(dir.join(rel), content).unwrap();
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn valid_composition_resolves_every_ref() {
    let g = parse_composition(&fixture("valid")).expect("valid fixture should resolve");
    for path in [
        "bpm",
        "meter",
        "master",
        "bass",
        "lead-dry",
        "lead",
        "filt",
        "drums/kick",
        "drums/snare",
        "drums/pattern-1b",
        "saw-series",
        "analytic",
        "tilt",
        "click",
        "step-dual",
    ] {
        assert!(g.expr(path).is_some(), "missing node for {path}");
    }
}

#[test]
fn the_frequency_variable_the_imaginary_unit_and_an_infinite_bound_are_plain_names() {
    for (src, name) in [("f", "f"), ("i", "i"), ("inf", "inf")] {
        assert_eq!(parse_expr(src).unwrap(), Expr::Var(name.to_string()));
    }
}

/// A series is a value, never the terms it would expand to.
#[test]
fn a_series_parses_to_one_call_carrying_its_inf_bound() {
    let Expr::Call { name, args, .. } = parse_expr("sum(k, 1, inf, sin(2*pi*k*220*t)/k)").unwrap()
    else {
        panic!("a series should parse as a call");
    };
    assert_eq!(name, "sum");
    assert_eq!(args.len(), 4);
    assert_eq!(args[2], Arg::Pos(Expr::Var("inf".to_string())));
}

#[test]
fn the_singular_constructors_parse_as_ordinary_calls() {
    for (src, name, arity) in [
        ("delta(t - 1s)", "delta", 1),
        ("delta(t, k=1)", "delta", 2),
        ("pv(2*pi*i*f)", "pv", 1),
    ] {
        let Expr::Call {
            name: parsed, args, ..
        } = parse_expr(src).unwrap()
        else {
            panic!("{src} should parse as a call");
        };
        assert_eq!(parsed, name);
        assert_eq!(args.len(), arity, "{src}");
    }
}

/// A cycle and a dangling ref are refused at the composition boundary, where an author meets
/// them, rather than only inside the loader.
#[test]
fn cross_file_cycle_fixture_refuses() {
    let errs = parse_composition(&fixture("cross-file-cycle")).unwrap_err();
    assert!(errs.iter().any(|r| r.code == DiagCode::RefCycle));
}

#[test]
fn dangling_ref_fixture_refuses() {
    let errs = parse_composition(&fixture("dangling-ref")).unwrap_err();
    assert!(errs.iter().any(|r| r.code == DiagCode::DanglingRef));
}

#[test]
fn missing_file_fixture_refuses() {
    let errs = parse_composition(&fixture("missing-file")).unwrap_err();
    assert!(errs.iter().any(|r| r.code == DiagCode::DanglingRef));
}

#[test]
fn malformed_expression_fixture_refuses() {
    let errs = parse_composition(&fixture("malformed")).unwrap_err();
    assert!(errs.iter().any(|r| r.code == DiagCode::UnexpectedEof));
}

#[test]
fn tsv_without_a_span_suffix_fixture_refuses() {
    let errs = parse_composition(&fixture("tsv-no-span")).unwrap_err();
    assert!(errs.iter().any(|r| r.code == DiagCode::TsvMissingSpan));
}

/// A default is arithmetic a caller may omit, not a second way to name a signal.
#[test]
fn a_default_line_is_read_off_the_top_of_a_file_and_bounded_to_arithmetic() {
    let dir = tmp("defaults");
    write(&dir, "song", "vel = 0.8\nsin(2*pi*220*t)*vel\n");
    let g = parse_composition(&dir).unwrap();
    assert_eq!(g.defaults("song").len(), 1);
    assert_eq!(g.defaults("song")[0].0, "vel");
    assert_eq!(
        g.expr("song"),
        parse_composition(&{
            let plain = tmp("defaults-plain");
            write(&plain, "song", "sin(2*pi*220*t)*vel\n");
            plain
        })
        .unwrap()
        .expr("song"),
        "the body is read exactly as a file with no defaults is"
    );

    for (content, why) in [
        ("g = 1\ng = 2\nt*g\n", "two defaults for one name"),
        ("g = 1\nsin(t)\n", "the body never reads it"),
    ] {
        let d = tmp("defaults-bad");
        write(&d, "song", content);
        let refusals = parse_composition(&d).unwrap_err();
        assert_eq!(
            refusals[0].code,
            DiagCode::BadDefault,
            "{why}: {}",
            refusals[0].reason
        );
    }
}

/// A name no ref can spell is no node, whatever it holds: the walk passes over it and says
/// which. A name a ref could spell is read as a node, and a non-text one refuses by name.
#[test]
fn a_file_that_is_not_text_refuses_naming_what_a_composition_holds() {
    let dir = tmp("not-text");
    write(&dir, "song", "sin(2*pi*220*t)\n");
    std::fs::write(dir.join("out.wav"), [0x00u8, 0xff, 0xfe]).unwrap();
    let g = parse_composition(&dir).expect("a rendering beside a node is not a node");
    assert_eq!(
        g.skipped(),
        [Skipped {
            path: "out.wav".to_string(),
            reason: Skip::Unnameable
        }],
        "the file is named, not parsed"
    );

    let named = tmp("not-text-named");
    write(&named, "song", "sin(2*pi*220*t)\n");
    std::fs::write(named.join("out"), [0x00u8, 0xff, 0xfe]).unwrap();
    let refusals = parse_composition(&named).unwrap_err();
    assert_eq!(refusals[0].code, DiagCode::Io);
    assert_eq!(refusals[0].at.path, "out");
    assert!(
        refusals[0].reason.contains("is not text"),
        "{}",
        refusals[0].reason
    );
    assert!(refusals[0].reason.contains("read as a node"));

    let instance = tmp("not-text-instance");
    write(&instance, "song", "sin(2*pi*220*t)\n");
    write(&instance, "f(k=2)", "1\n");
    let g = parse_composition(&instance).expect("an instance-shaped name is a node");
    assert!(
        g.skipped().is_empty(),
        "a render names it even though no ref spells it: {:?}",
        g.skipped()
    );
    assert!(g.expr("f(k=2)").is_some(), "and it parses as one");
}

/// A parameter is read where it is written, and a call is written the same as a name.
#[test]
fn a_defaulted_parameter_called_with_a_shift_is_mentioned() {
    let dir = tmp("called-default");
    write(&dir, "song", "p = 0\nd = 0.01s\np(t - d)\n");
    let g = parse_composition(&dir).expect("a called parameter is read");
    assert!(g.expr("song").is_some(), "the node parses");

    let shadow = tmp("called-default-shadow");
    write(&shadow, "song", "crop = 5\ncrop(sin(t), 0s, 1s)\n");
    parse_composition(&shadow)
        .expect("the grammar knows no vocabulary; the engine refuses the shadowed name");

    let dead = tmp("called-default-dead");
    write(&dead, "song", "p = 0\nq = 1\np(t)\n");
    let refusals = parse_composition(&dead).unwrap_err();
    assert_eq!(
        refusals[0].code,
        DiagCode::BadDefault,
        "a name neither written nor called stays dead: {}",
        refusals[0].reason
    );
}

/// A cell is a closed form in its row's own local time, so the window a cell writes must land where
/// the cell's voice lands. FORMAT 15.4's placement is the one `concat` writes: shift both.
#[test]
fn a_cropped_cell_moves_with_its_row() {
    let dir = tmp("cropped-cell");
    write(&dir, "kick", "sin(2*pi*50*t)\n");
    write(
        &dir,
        "pattern-2b",
        "crop(@kick(t), 0s, 0.5s)\ncrop(@kick(t), 0s, 0.5s)\n",
    );
    let mut g = parse_composition(&dir).expect("the grid should resolve");
    g.resolve_bar_spans(2.0);

    let placed = sva_ast::render_expr(g.expr("pattern-2b").expect("a materialized grid"));
    assert_eq!(
        placed, "crop(@kick, 0s, 0.5s) + crop(@kick(t - 2s), 2s, 2.5s)",
        "row 1 of 2 sits two seconds into a four-second span, window and voice together"
    );
}

/// A default line reads too, so a name the lines below it use is read. Liveness runs back
/// from the body: a name only a dead default names is dead with it.
#[test]
fn a_default_only_a_dead_default_reads_is_dead_with_it() {
    let dir = tmp("dead-default-chain");
    write(&dir, "live", "a = 1\nb = a + 1\nb*sin(2*pi*440*t)\n");
    write(&dir, "dead", "a = 1\nb = a + 1\nsin(2*pi*440*t)\n");

    parse_composition(&dir).expect_err("`b` reaches no body, so neither does `a`");

    let dir = tmp("live-default-chain");
    write(&dir, "live", "a = 1\nb = a + 1\nb*sin(2*pi*440*t)\n");
    parse_composition(&dir).expect("a chain the body reads is read");
}

/// A dot directory belongs to a tool, and a repository's own internals are neither nodes nor
/// worth opening: the walk never descends into one.
#[test]
fn a_dot_directory_is_never_walked() {
    let dir = tmp("dot-directory");
    write(&dir, "master", "sin(2*pi*220*t)\n");
    std::fs::create_dir_all(dir.join(".git/hooks")).unwrap();
    write(&dir, ".git/HEAD", "ref: refs/heads/master\n");
    write(&dir, ".git/hooks/pre-commit.sample", "#!/bin/sh\n");
    write(&dir, ".gitignore", "target\n");

    let paths = sva_ast::Dir::at(&dir)
        .paths()
        .expect("a readable directory");
    assert!(
        paths.found.iter().all(|p| !p.starts_with(".git/")),
        "nothing under a dot directory is listed: {paths:?}"
    );
    assert!(
        paths.found.contains(&".gitignore".to_string()),
        "a dot file at the top is still a file the lint speaks about: {paths:?}"
    );
}
