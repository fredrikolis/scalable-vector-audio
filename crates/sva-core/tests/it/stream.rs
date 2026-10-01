// Concern: states a stream change reads off its source only the nodes it newly reads | Non-concern: what the stream plays (sva-engine's suites) | IO: (a source, changes) -> what each read and took in

use std::borrow::Cow;
use std::cell::RefCell;

use sva_ast::{Composition, Listing, Source};
use sva_core::{Job, Placed, Stream};
use sva_engine::NoStore;

/// A composition counting each node text it hands out.
struct Counted {
    held: Composition,
    read: RefCell<Vec<String>>,
}

impl Source for Counted {
    fn paths(&self) -> Result<Listing, String> {
        self.held.paths()
    }

    fn get(&self, path: &str) -> Result<Option<Cow<'_, str>>, String> {
        let text = self.held.get(path)?;
        if text.is_some() {
            self.read.borrow_mut().push(path.to_string());
        }
        Ok(text)
    }
}

/// A future over no store: one poll finishes it.
fn now<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = std::task::Waker::noop();
    match future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(waker))
    {
        std::task::Poll::Ready(out) => out,
        std::task::Poll::Pending => panic!("nothing to wait on over no store"),
    }
}

fn source() -> Counted {
    let mut held = Composition::new();
    held.insert(
        "blip",
        "crop(lowpass(sample(sin(2*pi*f0*t)), cutoff=2000), 0s, 0.02s)\n",
    );
    held.insert("tone", "0.1*sin(2*pi*220*t)\n");
    held.insert("bed", "0.2*@tone\n");
    Counted {
        held,
        read: RefCell::default(),
    }
}

fn add(stream: &RefCell<Stream>, source: &Counted, term: &str) -> Vec<String> {
    source.read.borrow_mut().clear();
    let at = (term, Placed::Written);
    now(sva_core::add(stream, source, at, &NoStore)).unwrap_or_else(|e| panic!("{e:?}"));
    source.read.borrow().clone()
}

/// The first term reading a node reads it, once; each later one reads nothing, taking in its
/// own text alone.
#[test]
fn an_add_reads_off_its_source_only_what_the_stream_lacks() {
    let source = source();
    let job = Job::over(&source, "@notes + @bed");
    let stream = now(sva_core::stream(&job, (64, None), &NoStore)).expect("a stream");
    let stream = RefCell::new(stream);
    assert_eq!(add(&stream, &source, "@blip(t, f0=100)"), ["blip"]);
    assert_eq!(stream.borrow().counts().built.parsed, 2);
    for k in 1..4 {
        let term = format!("@blip(t - {}sp, f0={})", 64 * k, 100 + k);
        assert_eq!(add(&stream, &source, &term), Vec::<String>::new());
        assert_eq!(stream.borrow().counts().built.parsed, 1, "term {k}");
    }
    assert_eq!(
        add(&stream, &source, "@tone(t - 9sp)"),
        Vec::<String>::new()
    );
}
