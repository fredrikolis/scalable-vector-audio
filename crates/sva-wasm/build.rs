// Concern: sizes the stack the wasm module links with | Non-concern: what recurses on it (sva-engine's lower/render) | IO: (TARGET) -> a link argument

/// Parsing, lowering and dropping a node's expression each recurse to its depth, and `sum` lets
/// an author choose that depth. A native render gets a 128 MB thread stack; a wasm module has
/// only what it links with, and wasm-ld's default 1 MB overflows on the first real composition.
const STACK_BYTES: usize = 32 << 20;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo::rustc-link-arg=-zstack-size={STACK_BYTES}");
    }
}
