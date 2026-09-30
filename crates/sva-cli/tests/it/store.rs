// Concern: proves a render keeps what it computed in a store on a path, read back next time | Non-concern: eviction and versions (sva-engine's suite) | IO: (argv, env) -> a store on disk, a log

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::helpers::{put, scratch};

fn run(dir: &Path, args: &[&str], xdg: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sva-cli"));
    command
        .current_dir(dir)
        .args(["render"])
        .args(args)
        .env("SVA_LOG", "debug");
    if let Some(xdg) = xdg {
        command.env("XDG_CACHE_HOME", xdg);
    }
    command.output().expect("the binary runs")
}

/// Every count on the log's `total` line, by name.
fn total(out: &Output, name: &str) -> usize {
    let log = String::from_utf8_lossy(&out.stderr);
    let line = log
        .lines()
        .find(|l| l.starts_with("sva-cache total"))
        .unwrap_or_else(|| panic!("a total line: {log}"));
    line.split(' ')
        .find_map(|field| field.strip_prefix(&format!("{name}=")))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{name} on {line}"))
}

fn values_in(store: &Path) -> usize {
    std::fs::read_dir(store)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().len() == 32)
                .count()
        })
        .unwrap_or(0)
}

/// The envelope before its `meta`, which differs per run.
fn data(out: &Output) -> String {
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    printed[..printed.find("\"meta\"").expect("an envelope")].to_string()
}

fn shm(name: &str) -> PathBuf {
    let path = Path::new("/dev/shm").join(format!("sva-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

#[cfg(target_os = "linux")]
#[test]
fn a_store_under_dev_shm_answers_the_next_render_with_every_value() {
    let dir = scratch("store-shm");
    put(&dir, "x", "crop(sample(sin(2*pi*220*t)), 0s, 0.05s)*0.5\n");
    let store = shm("warm");
    let args = ["@x", "--representation", "loudness", "--cache"];
    let args: Vec<&str> = args.into_iter().chain([store.to_str().unwrap()]).collect();
    let cold = run(&dir, &args, None);
    assert!(
        cold.status.success(),
        "{}",
        String::from_utf8_lossy(&cold.stdout)
    );
    assert!(total(&cold, "store-miss") > 0);
    assert!(values_in(&store) > 0, "the render's values are on disk");

    let warm = run(&dir, &args, None);
    assert_eq!(total(&warm, "miss"), 0, "nothing is computed");
    assert_eq!(total(&warm, "store-miss"), 0);
    assert!(total(&warm, "store-hit") > 0);
    assert_eq!(data(&cold), data(&warm));
    let _ = std::fs::remove_dir_all(&store);
}

/// A sample past what a float holds refuses only once the render has computed it.
#[test]
fn a_render_that_fails_after_computing_still_stores_what_it_computed() {
    let dir = scratch("store-fails");
    put(
        &dir,
        "loud",
        "crop(sample(sin(2*pi*220*t)), 0s, 0.05s)*1e40\n",
    );
    let out = scratch("store-fails-out").join("loud.wav");
    let store = scratch("store-fails-store");
    let asked = format!("samples={}", out.display());
    let args = [
        "@loud",
        "--representation",
        &asked,
        "--cache",
        store.to_str().unwrap(),
    ];
    let failed = run(&dir, &args, None);
    assert_eq!(failed.status.code(), Some(3), "the sample refuses");
    assert!(values_in(&store) > 0, "what it computed is stored");
    assert!(total(&run(&dir, &args, None), "store-hit") > 0);
}

#[test]
fn the_default_store_is_under_xdg_cache_home_and_none_keeps_none() {
    let dir = scratch("store-default");
    put(&dir, "x", "crop(sin(2*pi*220*t), 0s, 0.05s)\n");
    let (on, off) = (scratch("store-xdg-on"), scratch("store-xdg-off"));
    assert!(
        run(&dir, &["@x", "--representation", "loudness"], Some(&on))
            .status
            .success()
    );
    assert!(on.join("sva").join("index").is_file());
    let args = ["@x", "--representation", "loudness", "--cache", "none"];
    assert!(run(&dir, &args, Some(&off)).status.success());
    assert!(!off.join("sva").exists(), "`none` keeps no store");
}

/// A staging area is removed once no process holds its lock, and kept while one does.
#[test]
fn a_staging_area_no_process_holds_is_swept_and_a_held_one_survives() {
    let dir = scratch("store-sweep");
    put(&dir, "x", "crop(sample(sin(2*pi*220*t)), 0s, 0.05s)*0.5\n");
    let store = scratch("store-sweep-store");
    let stale = store.join(".staging-4000000001");
    std::fs::create_dir_all(&stale).expect("a stale area");
    std::fs::write(stale.join("chunk"), b"left behind").expect("a chunk");
    let live = store.join(".staging-4000000002");
    std::fs::create_dir_all(&live).expect("a live area");
    let lock = std::fs::File::create(store.join(".staging-4000000002.lock")).expect("a lock");
    lock.lock().expect("held");
    let args = ["@x", "--representation", "loudness", "--cache"];
    let args: Vec<&str> = args.into_iter().chain([store.to_str().unwrap()]).collect();
    assert!(run(&dir, &args, None).status.success());
    assert!(!stale.exists(), "no process held it");
    assert!(live.exists(), "this test holds it");
}

/// A cache that cannot be used fails no render: the render answers as with `--cache none`, and
/// a warning says why it kept nothing.
#[test]
fn a_render_over_an_unusable_cache_answers_and_logs_why() {
    let dir = scratch("store-unusable");
    put(&dir, "x", "crop(sample(sin(2*pi*220*t)), 0s, 0.05s)*0.5\n");
    let file = scratch("store-unusable-file").join("not-a-directory");
    std::fs::write(&file, b"a file").expect("a file");
    let args = ["@x", "--representation", "loudness", "--cache"];
    let over: Vec<&str> = args.into_iter().chain([file.to_str().unwrap()]).collect();
    let none: Vec<&str> = args.into_iter().chain(["none"]).collect();
    let (failed, bare) = (run(&dir, &over, None), run(&dir, &none, None));
    assert!(
        failed.status.success(),
        "{}",
        String::from_utf8_lossy(&failed.stdout)
    );
    let before = |out: &Output| {
        let data = data(out);
        data[..data.find("\"diagnostics\"").expect("diagnostics")].to_string()
    };
    assert_eq!(before(&failed), before(&bare));
    let printed = String::from_utf8_lossy(&failed.stdout);
    assert!(
        printed.contains("\"code\": \"store.unusable\", \"severity\": \"warning\""),
        "{printed}"
    );
}
