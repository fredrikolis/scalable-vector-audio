// Concern: proves `outline` prints the engine's own parse tree of an expression, spans and all | Non-concern: the tree itself (sva-ast's suite) | IO: (argv) -> an envelope

use std::process::Command;

fn outline(text: &str) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sva-cli"))
        .args(["outline", text])
        .output()
        .expect("the binary runs");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

#[test]
fn outline_prints_each_node_beside_its_bytes() {
    let text = "0.5*chaigne_askenfelt(f0, b=max(1.4e-4, 2))";
    let (code, printed) = outline(text);
    assert_eq!(code, Some(0), "{printed}");
    for field in [
        "\"kind\": \"operator\", \"span\": { \"start\": 0, \"end\": 43 }",
        "\"op\": \"*\", \"at\": { \"start\": 3, \"end\": 4 }",
        "\"name\": \"chaigne_askenfelt\", \"at\": { \"start\": 4, \"end\": 21 }",
        "\"kind\": \"named\", \"name\": \"b\", \"at\": { \"start\": 26, \"end\": 27 }",
        "\"kind\": \"literal\", \"span\": { \"start\": 32, \"end\": 38 }, \"written\": true, \
         \"unit\": \"number\", \"value\": 0.00014",
    ] {
        assert!(printed.contains(field), "{field}: {printed}");
    }
}

#[test]
fn outline_refuses_at_the_byte_the_parser_stopped() {
    let (code, printed) = outline("sin(");
    assert_ne!(code, Some(0), "{printed}");
    assert!(
        printed.contains("\"code\": \"unexpected-eof\""),
        "{printed}"
    );
    assert!(printed.contains("\"offset\": 4"), "{printed}");
}
