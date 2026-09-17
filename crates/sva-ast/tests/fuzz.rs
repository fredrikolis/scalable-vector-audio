// Concern: proves parse_expr and parse_composition never panic on hostile input | Non-concern: what any input evaluates to, the internal modules | IO: (hostile &str / &Path) -> no panic

use std::fs;
use std::path::Path;
use sva_ast::parse_expr;

const HOSTILE: &[&str] = &[
    "",
    "@",
    "@@@@",
    "(((((",
    ")))))",
    "self",
    "self(",
    "self()",
    ".....",
    "1 + + + +",
    "@a(@b(@c(@d(@e(t)))))",
    "\0\0\0",
    "λλλ",
    "\t\t\t",
    "t.t.t.t.t.t.t.t.t.t.t.t.t.t.t.t.t.t.t.t",
    "sin(sin(sin(sin(sin(sin(sin(sin(t",
    "=====",
    "1,2,3",
    "1.2.3.4",
    "😀😀😀",
];

#[test]
fn hostile_expression_text_never_panics() {
    for s in HOSTILE {
        let _ = parse_expr(s);
    }
}

#[test]
fn truncated_and_deeply_nested_never_panics() {
    for depth in [1, 10, 100, 1000, 10_000] {
        let deep_parens = format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
        let _ = parse_expr(&deep_parens);
        let deep_refs = format!("{}t{}", "self(".repeat(depth), ")".repeat(depth));
        let _ = parse_expr(&deep_refs);
        let truncated = &deep_parens[..deep_parens.len() / 2];
        let _ = parse_expr(truncated);
    }
}

#[test]
fn a_deep_cross_file_ref_chain_never_panics_or_hangs() {
    let dir = std::env::temp_dir().join(format!("sva-ast-fuzz-refchain-{:x}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    const N: usize = 20_000;
    for i in 0..N {
        let content = format!("@node{}\n", i + 1);
        fs::write(dir.join(format!("node{i}")), content).unwrap();
    }
    fs::write(dir.join(format!("node{N}")), "1\n").unwrap();

    let _ = sva_ast::parse_composition(&dir);

    fs::write(dir.join(format!("node{N}")), "@node0\n").unwrap();
    let _ = sva_ast::parse_composition(&dir);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_nonexistent_composition_directory_refuses_not_panics() {
    let _ = sva_ast::parse_composition(Path::new("/does/not/exist/anywhere"));
}
