// Concern: fingerprints a crate's own sources and folds fingerprints into one | Non-concern: what a fingerprint keys (sva-engine) | IO: (dir) -> u64, (&[&str]) -> u64

use std::path::{Path, PathBuf};

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn source_hash(dir: &Path) -> u64 {
    let mut files = Vec::new();
    collect(dir, &mut files);
    files.sort();
    let mut h = OFFSET;
    for file in &files {
        absorb(&mut h, file.to_string_lossy().as_bytes());
        absorb(
            &mut h,
            &std::fs::read(file).expect("a listed source file is readable"),
        );
    }
    h
}

pub const fn fold(parts: &[&str]) -> u64 {
    let mut h = OFFSET;
    let mut i = 0;
    while i < parts.len() {
        let bytes = parts[i].as_bytes();
        let mut j = 0;
        while j <= bytes.len() {
            let b = if j < bytes.len() { bytes[j] } else { 0xff };
            h = (h ^ b as u64).wrapping_mul(PRIME);
            j += 1;
        }
        i += 1;
    }
    h
}

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
        *h = (*h ^ u64::from(b)).wrapping_mul(PRIME);
    }
}
