// Concern: states a stream change reads off its source only the nodes it newly reads | Non-concern: what the stream plays (sva-engine's suites) | IO: (a source, changes) -> what each read and took in

use std::borrow::Cow;
use std::cell::RefCell;

use sva_ast::{Composition, Listing, Source};
use sva_core::{Job, Placed, Stream};
use sva_engine::Tier;

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

    fn generation(&self, path: &str) -> Option<u64> {
        self.held.generation(path)
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
    now(sva_core::add(stream, source, at, &Tier::default())).unwrap_or_else(|e| panic!("{e:?}"));
    source.read.borrow().clone()
}

/// The first term reading a node reads it, once; each later one reads nothing, taking in its
/// own text alone.
#[test]
fn an_add_reads_off_its_source_only_what_the_stream_lacks() {
    let source = source();
    let job = Job::over(&source, "@notes + @bed");
    let stream = now(sva_core::stream(&job, (64, None), &Tier::default())).expect("a stream");
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

/// A node written again as it was is read once more, at the next change, and not after.
#[test]
fn a_node_written_again_as_it_was_is_read_at_the_next_change_alone() {
    let mut source = source();
    let job = Job::over(&source, "@notes + @bed");
    let stream = now(sva_core::stream(&job, (64, None), &Tier::default())).expect("a stream");
    let stream = RefCell::new(stream);
    source.held.insert("tone", "0.1*sin(2*pi*220*t)\n");
    assert_eq!(add(&stream, &source, "@tone(t - 9sp)"), ["tone"]);
    assert_eq!(
        add(&stream, &source, "@tone(t - 18sp)"),
        Vec::<String>::new()
    );
}

/// An edit whose held node now reads a node the stream never held takes that node in, and
/// plays from there the samples a render of the edited source writes.
#[test]
fn an_edit_takes_in_a_node_a_held_node_newly_reads() {
    let mut held = Composition::new();
    held.insert("a", "0.1*sin(2*pi*220*t)\n");
    held.insert("master", "@a\n");
    let job = Job::over(&held, "@master");
    let stream = now(sva_core::stream(&job, (64, None), &Tier::default())).expect("a stream");
    let stream = RefCell::new(stream);
    stream.borrow_mut().read(0, 64).expect("a block");
    held.insert("b", "0.1*sin(2*pi*330*t)\n");
    held.insert("master", "@a + @b\n");
    now(sva_core::edit(&stream, &held, "@master", &Tier::default())).expect("an edit");
    let at = stream.borrow().position();
    let block = stream.borrow_mut().read(at, 64).expect("a block");
    let played = block.expect("samples").plane(0).to_vec();
    let job = Job::over(&held, "@master([0, 0.01s])");
    let rendered = sva_core::execute(job, &Tier::default()).expect("a render");
    let whole = rendered
        .render
        .output(rendered.render.root)
        .expect("the root");
    let span = at as usize..at as usize + 64;
    assert_eq!(played, whole.plane(0)[span].to_vec());
}
