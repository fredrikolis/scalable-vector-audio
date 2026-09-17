// Concern: runs a call on its own stack and turns whatever stopped it into one response | Non-concern: what the call does, the envelope's shape (sva-core) | IO: (a call) -> its value or why it stopped

use std::sync::{Arc, Mutex};

use sva_core::{Diagnostic, error_envelope};

#[derive(Debug)]
pub enum Stopped {
    Panicked(String),
    NoWorker(String),
}

/// The default hook leaves a stack trace on stderr and 101, not a JSON response.
pub fn caught<T: Send + 'static>(
    stack_bytes: usize,
    call: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Stopped> {
    let held = Arc::new(Mutex::new(String::new()));
    let seen = Arc::clone(&held);
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        *seen.lock().unwrap_or_else(|e| e.into_inner()) = info.to_string();
    }));
    let ran = match std::thread::Builder::new()
        .stack_size(stack_bytes)
        .spawn(call)
    {
        Ok(worker) => worker
            .join()
            .map_err(|_| Stopped::Panicked(held.lock().unwrap_or_else(|e| e.into_inner()).clone())),
        Err(e) => Err(Stopped::NoWorker(format!(
            "no thread with a {stack_bytes}-byte stack: {e}"
        ))),
    };
    std::panic::set_hook(before);
    ran
}

/// One `internal_error` either way, under the code and the advice that fit what stopped it.
pub fn stopped(why: &Stopped) -> String {
    let (message, code, text, help) = match why {
        Stopped::Panicked(text) => (
            "sva-cli panicked",
            "cli.panicked",
            text,
            "this is a defect in sva-cli, not in the composition; report it with the command \
             that reached it",
        ),
        Stopped::NoWorker(text) => (
            "sva-cli could not start its worker",
            "cli.no_worker",
            text,
            "the machine refused the thread this CLI runs on; raise the process's memory or \
             thread limit and run it again",
        ),
    };
    error_envelope(
        "internal_error",
        message,
        &[Diagnostic::new(code, text.clone()).helped(help)],
    )
}
