// Concern: proves one instance reports the parameters its invocation resolved, not its file's defaults | Non-concern: which instances a composition has | IO: (a composition, path) -> Vec<Binding>

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{
    Ask, Binding, EngineError, Output, Render, RenderConfig, Representation, answer, render,
};

fn rendered(g: &Graph) -> Render {
    let asks = vec![Ask {
        node: "song".to_string(),
        representation: Representation::Bindings,
    }];
    render(
        g,
        "song",
        RenderConfig::seconds(8_000, 0.25).asking(asks),
        None,
    )
    .expect("a composition that renders")
}

fn bindings(held: &Render, path: &str) -> Result<Vec<Binding>, EngineError> {
    let id = held.node(path)?;
    match answer(held, id, Representation::Bindings)?.value {
        Output::Bindings(held) => Ok(held),
        other => panic!("expected the resolved parameters, got {other:?}"),
    }
}

/// The bug this closes: `f` defaults `k` to 1 and `song` passes `k=2`.
#[test]
fn an_explicit_override_reaches_the_instance_rather_than_the_files_own_default() {
    let g = graph_of(
        "override",
        &[
            ("src", "sin(t)\n"),
            ("f", "k = 1\nx*k\n"),
            ("song", "@f(t, x=@src, k=2)\n"),
        ],
    );
    let held = rendered(&g);
    let bound = bindings(&held, "f(k=2, x=@src)").expect("the instance");
    let k = bound.iter().find(|b| b.name == "k").expect("k");
    assert_eq!(k.source, "2", "the override must win, not the default of 1");
    let x = bound.iter().find(|b| b.name == "x").expect("x");
    assert_eq!(x.source, "@src");
}

#[test]
fn an_unknown_path_refuses() {
    let g = graph_of("missing", &[("song", "sin(t)\n")]);
    let held = rendered(&g);
    assert!(matches!(
        bindings(&held, "nope"),
        Err(EngineError::UnknownNode(p)) if p == "nope"
    ));
}
