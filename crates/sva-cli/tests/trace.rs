// Concern: states what a trace must answer about a node's position, whatever the implementation | Non-concern: how any pass computes it (sva-engine) | IO: (fixtures/*) -> asserted Traced or CliError

mod helpers;

use std::path::{Path, PathBuf};

use helpers::{put, scratch};
use sva_cli::{CliError, Traceable, trace, trace_data};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn traced(name: &str, target: &str) -> Traceable {
    trace(&fixture(name), target)
        .unwrap_or_else(|e| panic!("tracing `{target}` in {name}: {}", e.message()))
}

fn up(t: &Traceable) -> Vec<&str> {
    t.traced.up.iter().map(|u| u.node.as_str()).collect()
}

#[test]
fn down_is_one_hop_and_up_reaches_every_reader_to_an_entry_point() {
    let t = traced("reverb", "src");
    assert_eq!(t.traced.down, Vec::<String>::new(), "`src` reads nothing");
    assert_eq!(t.traced.entry, ["master"], "the only unreferenced node");
    let readers = up(&t);
    assert!(readers.contains(&"master"), "{readers:?}");
    assert!(
        readers.iter().filter(|n| n.starts_with("fx/comb(")).count() == 8,
        "every comb reads `src` through one invocation: {readers:?}"
    );
    assert!(
        readers.iter().any(|n| n.starts_with("fx/diffuse(")),
        "a reader four hops up is still a reader: {readers:?}"
    );
    assert!(
        !readers.contains(&"src"),
        "a node is not its own reader: {readers:?}"
    );
}

/// `from == node` is the whole marker, and there is no `self` field.
#[test]
fn a_self_read_names_the_node_in_its_own_down() {
    let comb = up(&traced("reverb", "src"))
        .into_iter()
        .find(|n| n.starts_with("fx/comb("))
        .expect("a comb instance")
        .to_string();
    let t = traced("reverb", &comb);
    assert!(t.traced.down.contains(&comb), "{:?}", t.traced.down);
    assert_eq!(t.traced.file.as_deref(), Some("fx/comb"));
}

#[test]
fn a_file_several_tuples_share_refuses_naming_them() {
    let Err(CliError::Engine(e)) = trace(&fixture("reverb"), "fx/comb") else {
        panic!("a file with eight instances must refuse")
    };
    let message = e.to_string();
    assert!(message.contains("8 instances"), "{message}");
    assert!(message.contains("fx/comb(delay="), "{message}");
}

/// Nothing references `probe`, so the forward question answers empty rather than refusing.
#[test]
fn an_expression_target_is_a_probe_nothing_reads() {
    let t = traced("reverb", "lowpass(@src, 400, 1)");
    assert_eq!(t.traced.node, "probe");
    assert_eq!(t.traced.down, ["src"]);
    assert!(t.traced.up.is_empty());
    assert!(t.traced.entry.contains(&"probe".to_string()));
}

/// A trace answers for the whole composition, never for one root: a node two entry points
/// reach is listed under both.
#[test]
fn a_second_entry_point_is_a_reader_like_any_other() {
    let t = traced("two-entries", "tail");
    assert_eq!(t.traced.entry, ["bench-2s", "master"]);
    assert_eq!(up(&t), ["bench-2s", "master"]);
}

#[test]
fn one_target_traces_to_the_same_bytes_every_time() {
    assert_eq!(
        trace_data(&traced("stereo", "src")),
        trace_data(&traced("stereo", "src"))
    );
}

/// FORMAT 15.3: a ref naming one number is that number wherever it is written, so a key in
/// a frequency position cannot type one way and read the other.
#[test]
fn a_key_ref_inside_sin_traces_with_a_dual() {
    let dir = scratch("key-in-sin");
    let head = "; Models: a probe | Neglects: everything | IO: () -> hertz | Tags: probe\n";
    put(&dir, "variables/key", &format!("{head}146.8324\n"));
    for (name, body) in [
        ("bare", "sin(2*pi*293.6648*t)"),
        ("keyed", "sin(2*pi*@variables/key*12st*t)"),
        ("scaled", "sin(2*pi*(@variables/key*1)*2*t)"),
        ("offset", "sin(2*pi*(@variables/key+0)*2*t)"),
    ] {
        put(
            &dir,
            name,
            &format!(
                "; Models: {name} | Neglects: everything | IO: (t) -> amplitude | Tags: probe\n{body}\n"
            ),
        );
    }

    let bare = trace(&dir, "bare").expect("a literal frequency").traced.ty;
    assert_eq!(
        bare, "a closed form in `t` with a dual",
        "a sine of a number keeps its dual"
    );
    for name in ["keyed", "scaled", "offset"] {
        let found = trace(&dir, name)
            .unwrap_or_else(|e| panic!("tracing `{name}`: {}", e.message()))
            .traced;
        assert_eq!(
            found.ty, bare,
            "`{name}` writes the key where `bare` writes the number it holds"
        );
    }
}

/// FORMAT 15.4: `repeat(@x, n)` is `n` copies of one ref, and a probe is argv math read in
/// this composition's namespace, so one text means one thing in both.
#[test]
fn repeat_desugars_like_concat() {
    let dir = scratch("repeat-probe");
    let head = "; Models: a probe | Neglects: everything | IO: (t) -> amplitude | Tags: probe\n";
    std::fs::create_dir_all(dir.join("variables")).expect("a variables directory");
    put(&dir, "variables/bpm", &format!("{head}120\n"));
    put(&dir, "variables/meter", &format!("{head}4/4\n"));
    put(&dir, "cell-1b", &format!("{head}sin(2*pi*440*t)\n"));
    put(&dir, "written", &format!("{head}repeat(@cell-1b, 3)\n"));

    let spelled = |target: &str| {
        trace(&dir, target)
            .unwrap_or_else(|e| panic!("tracing `{target}`: {}", e.message()))
            .traced
            .expr
    };

    let written = spelled("written");
    assert!(
        written.contains("crop(") && !written.contains("repeat("),
        "a file's repeat expands: {written}"
    );
    assert_eq!(
        spelled("repeat(@cell-1b, 3)"),
        written,
        "a probe reads the same text the same way"
    );
    assert_eq!(
        spelled("concat(@cell-1b, @cell-1b, @cell-1b)"),
        written,
        "`repeat(@x, n)` is `n` copies of one ref, spelled short"
    );
}

/// A bare file name is the file on its own terms, however many call sites bound it
/// otherwise: three verbs, one written name, one instance.
#[test]
fn three_verbs_agree_on_an_instance_name() {
    let dir = scratch("two-instances");
    let head = "; Models: a probe | Neglects: everything | IO: (t) -> amplitude | Tags: probe\n";
    put(
        &dir,
        "motif",
        &format!("{head}third = 15\nsin(2*pi*third*29*t)\n"),
    );
    put(
        &dir,
        "master",
        &format!("{head}@motif(t) + @motif(t, third=16)\n"),
    );

    let held = sva_core::execute(sva_core::Job {
        target: Some("motif"),
        ..sva_core::Job::over(&sva_ast::Dir::at(&dir))
    })
    .expect("`render motif` reads the file on its own terms");
    let rendered = held.render.tys.name(held.render.root).to_string();

    let traced = trace(&dir, "motif")
        .expect("`trace motif` reads the same file the same way")
        .traced;
    assert_eq!(traced.node, rendered, "one written name, one instance");
    assert_eq!(
        traced.node, "motif(third=15)",
        "the file's own defaults, not a call site's override"
    );
    assert_eq!(
        traced.entry,
        vec!["master".to_string()],
        "reading a file on its own terms does not make it an entry point"
    );

    let report = sva_cli::lint(&dir, Some("motif")).expect("`lint motif` names the same file");
    assert_eq!(report.nodes, 1, "one node checked");
}
