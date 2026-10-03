// Concern: states a front end's session renders type only what changed since the last and its readers | Non-concern: which nodes a change retypes | IO: (a source, renders) -> typed nodes

use std::collections::BTreeSet;

use sva_core::{Job, Session, execute_over};
use sva_engine::Tier;

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

fn typed(held: &sva_ast::Composition, session: &mut Session) -> BTreeSet<String> {
    let rendered = now(execute_over(
        Job::over(held, "@master([0, 0.05s])"),
        &Tier::default(),
        session,
    ));
    let render = rendered.unwrap_or_else(|e| panic!("{e}")).render;
    let stats = render.cache_stats.expect("a render over memory");
    stats.typed.into_iter().collect()
}

/// The source is read and settled anew each render; only what it holds otherwise is typed.
#[test]
fn a_session_types_only_what_its_source_changed_and_what_reads_it() {
    let mut held: sva_ast::Composition = [
        ("x", "sample(crop(sin(2*pi*220*t), 0s, 0.05s))\n"),
        ("y", "sample(crop(sin(2*pi*330*t), 0s, 0.05s))\n"),
        ("master", "@x*0.5 + @y\n"),
    ]
    .into_iter()
    .collect();
    let mut session = Session::default();
    let all: BTreeSet<String> = ["x", "y", "master", "probe"].map(String::from).into();
    assert_eq!(typed(&held, &mut session), all);
    assert_eq!(typed(&held, &mut session), BTreeSet::new());
    held.insert("x", "sample(crop(sin(2*pi*440*t), 0s, 0.05s))\n");
    let share: BTreeSet<String> = ["x", "master", "probe"].map(String::from).into();
    assert_eq!(typed(&held, &mut session), share);
}
