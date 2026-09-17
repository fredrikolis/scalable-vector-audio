// Concern: fingerprints this crate's sources, so a changed engine retires every cache entry | Non-concern: what a fingerprint keys (cache.rs) | IO: (src/**) -> SVA_ENGINE_SRC_HASH

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");

    let mut files = Vec::new();
    collect(Path::new("src"), &mut files);
    files.sort();

    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for file in &files {
        absorb(&mut h, file.to_string_lossy().as_bytes());
        absorb(
            &mut h,
            &std::fs::read(file).expect("a listed source file is readable"),
        );
    }
    println!("cargo:rustc-env=SVA_ENGINE_SRC_HASH={h:016x}");
}

/// A directory it could not walk is a fingerprint over less than the crate.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let read = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} is part of the fingerprint: {e}", dir.display()));
    for entry in read {
        let path = entry
            .unwrap_or_else(|e| panic!("an entry under {} is unreadable: {e}", dir.display()))
            .path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn absorb(h: &mut u64, bytes: &[u8]) {
    for &b in bytes.iter().chain(&[0xff]) {
        *h = (*h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
}
