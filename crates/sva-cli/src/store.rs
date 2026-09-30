// Concern: opens a render's one persistent store, persists it on a signal, warns of its failures | Non-concern: the medium (directory.rs), eviction | IO: (CacheAt) -> a Store or none, warnings

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;

use sva_core::{CliError, DEFAULT_STORE_BYTES, Diagnostic, Severity, Store, error_envelope};

use crate::args::CacheAt;
use crate::directory::Directory;

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The CLI's one executor: it parks the thread until the future wakes it.
pub fn wait<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut context) {
            return out;
        }
        std::thread::park();
    }
}

static OPEN: OnceLock<Store<Directory>> = OnceLock::new();

fn platform() -> Result<PathBuf, CliError> {
    let set = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    set("XDG_CACHE_HOME")
        .or_else(|| set("HOME").map(|home| home.join(".cache")))
        .map(|dir| dir.join("sva"))
        .ok_or_else(|| {
            CliError::Usage(
                "neither XDG_CACHE_HOME nor HOME is set, so there is no default cache \
                 directory; pass `--cache <path>` or `--cache none`"
                    .to_string(),
            )
        })
}

/// A store that fails fails no render: the render answers as without it, warned why.
pub fn warning(code: &str, message: String) -> Diagnostic {
    Diagnostic::new(code, message)
        .with_severity(Severity::Warning)
        .helped("pass `--cache <path>` elsewhere, or `--cache none` to keep no store")
}

/// The process's one store, opened once; a signal persists it before the process exits. One
/// that cannot open is none.
pub fn opened(
    at: &CacheAt,
    warnings: &mut Vec<Diagnostic>,
) -> Result<Option<&'static Store<Directory>>, CliError> {
    let path = match at {
        CacheAt::Off => return Ok(None),
        CacheAt::Path(path) => path.clone(),
        CacheAt::Platform => platform()?,
    };
    let backend = Directory::at(path.clone());
    let store = match wait(Store::open(backend, DEFAULT_STORE_BYTES)) {
        Ok(store) => OPEN.get_or_init(|| store),
        Err(why) => {
            let message = format!(
                "the cache at `{}` is unusable, so this render kept none: {why}",
                path.display()
            );
            warnings.push(warning("store.unusable", message));
            return Ok(None);
        }
    };
    match signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ]) {
        Ok(mut signals) => {
            std::thread::spawn(move || {
                if let Some(signal) = signals.forever().next() {
                    interrupted(store, signal);
                }
            });
        }
        Err(e) => warnings.push(warning(
            "store.unsignalled",
            format!("a signal would not have persisted this render: {e}"),
        )),
    }
    Ok(Some(store))
}

fn interrupted(store: &Store<Directory>, signal: i32) -> ! {
    let kept = match wait(store.persist()) {
        Ok(done) => format!("{} value(s) it computed were stored", done.written),
        Err(why) => format!("what it computed could not be stored: {why}"),
    };
    let message = format!("the render was stopped by signal {signal}; {kept}");
    let diagnostic = Diagnostic::new("cli.interrupted", message.clone())
        .helped("run the same render again to resume from what was stored");
    println!(
        "{}",
        error_envelope("internal_error", &message, &[diagnostic])
    );
    std::process::exit(1)
}

/// What a render left in the store's memory, written whatever the render came to.
pub fn persisted(store: &Store<Directory>, warnings: &mut Vec<Diagnostic>) {
    if let Err(why) = wait(store.persist()) {
        let message = format!("the render's values could not be stored: {why}");
        warnings.push(warning("store.unwritten", message));
    }
}
