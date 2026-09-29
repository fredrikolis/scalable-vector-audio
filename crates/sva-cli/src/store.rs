// Concern: opens a render's one persistent store and persists it on SIGINT or SIGTERM | Non-concern: the medium (directory.rs), eviction | IO: (CacheAt) -> a Store; (a signal) -> persist, exit

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;

use sva_core::{Cache, CliError, DEFAULT_STORE_BYTES, Diagnostic, Store, error_envelope};

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

fn unusable(path: &Path, why: &str) -> CliError {
    CliError::Io(format!(
        "the cache at `{}` is unusable: {why}; pass `--cache <path>` elsewhere or `--cache none`",
        path.display()
    ))
}

/// The process's one store, opened once; a signal persists it before the process exits.
pub fn opened(at: &CacheAt) -> Result<Option<&'static Store<Directory>>, CliError> {
    let path = match at {
        CacheAt::Off => return Ok(None),
        CacheAt::Path(path) => path.clone(),
        CacheAt::Platform => platform()?,
    };
    let backend = Directory { path: path.clone() };
    let store = wait(Store::open(backend, Cache::new(), DEFAULT_STORE_BYTES))
        .map_err(|why| unusable(&path, &why))?;
    let store = OPEN.get_or_init(|| store);
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ])
    .map_err(|e| unusable(&path, &e.to_string()))?;
    std::thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            interrupted(store, signal);
        }
    });
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
pub fn persisted(store: &Store<Directory>) -> Result<(), CliError> {
    wait(store.persist())
        .map(|_| ())
        .map_err(|why| CliError::Io(format!("the render's values could not be stored: {why}")))
}
