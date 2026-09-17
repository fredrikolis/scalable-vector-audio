// Concern: proves whatever stops this CLI still leaves one JSON response | Non-concern: what a subcommand does (its own suite) | IO: (a call that stops) -> an envelope

use std::sync::{Arc, Mutex};

use sva_cli::{Stopped, caught, stopped};

const STACK_BYTES: usize = 1 << 21;

/// A panic used to reach `resume_unwind`: a stack trace on stderr and 101, not a response.
#[test]
fn a_panic_under_the_cli_answers_an_internal_error_envelope() {
    let seen = Arc::new(Mutex::new(false));
    let mine = Arc::clone(&seen);
    std::panic::set_hook(Box::new(move |_| *mine.lock().expect("the flag") = true));

    assert_eq!(
        caught(STACK_BYTES, || 7).expect("a call that returns"),
        7,
        "a call that does not panic answers its own value"
    );

    let why = caught(STACK_BYTES, || -> u8 {
        panic!("a defect reached the grid")
    })
    .expect_err("a panic is caught, not re-raised");
    let Stopped::Panicked(text) = &why else {
        panic!("a panic, not {why:?}")
    };
    assert!(text.contains("a defect reached the grid"), "{text}");
    assert!(text.contains("panics.rs"), "and where it struck: {text}");
    assert!(!*seen.lock().expect("the flag"), "the inner hook took it");

    let _ = std::panic::catch_unwind(|| panic!("outside"));
    assert!(
        *seen.lock().expect("the flag"),
        "and the hook before it is back for the next one"
    );
    let _ = std::panic::take_hook();

    let answered = stopped(&why);
    for key in [
        "\"status\": \"error\"",
        "\"code\": \"internal_error\"",
        "cli.panicked",
        "a defect reached the grid",
        "\"diagnostics\"",
        "\"request_id\"",
    ] {
        assert!(
            answered.contains(key),
            "the envelope answers {key}: {answered}"
        );
    }
}

/// Only the compiled binary holds `main`'s failure arm, and a worker that never starts is the
/// one failure reaching it that no input can stage.
#[cfg(target_os = "linux")]
#[test]
fn the_binary_answers_an_envelope_where_its_worker_cannot_start() {
    let out = std::process::Command::new("bash")
        .arg("-c")
        // 100 MB of address space against the 128 MB stack `main` asks its worker for.
        .arg(format!(
            "ulimit -v 100000; exec '{}' render --as lines",
            env!("CARGO_BIN_EXE_sva-cli")
        ))
        .output()
        .expect("bash runs the binary under a memory bound");
    let answered = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "the standard's code for internal_error: {answered}"
    );
    for key in [
        "\"status\": \"error\"",
        "\"code\": \"internal_error\"",
        "cli.no_worker",
        "no thread with a",
    ] {
        assert!(
            answered.contains(key),
            "the envelope answers {key}: {answered}"
        );
    }
    assert!(
        out.stderr.is_empty(),
        "and nothing on stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
