// Concern: fingerprints the sources whose change can change a stored value's bits | Non-concern: wiping a store it no longer matches (cache/persist.rs) | IO: (the crates' src trees) -> SVA_ENGINE_BUILD

use std::path::{Path, PathBuf};

/// A sibling absent from a packaged build is skipped.
const SOURCES: [&str; 4] = [
    "src",
    "../sva-samples/src",
    "../sva-formula/src",
    "../sva-ast/src",
];

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|e| e.path()) {
        match path.is_dir() {
            true => files(&path, out),
            false => out.push(path),
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let mut found = Vec::new();
    for dir in SOURCES {
        let dir = root.join(dir);
        println!("cargo:rerun-if-changed={}", dir.display());
        files(&dir, &mut found);
    }
    found.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for path in found {
        let name = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let bytes = std::fs::read(&path).unwrap_or_default();
        for b in name.bytes().chain(bytes) {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    println!("cargo:rustc-env=SVA_ENGINE_BUILD={hash:016x}");
}
