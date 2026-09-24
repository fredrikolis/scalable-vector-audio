// Concern: fingerprints this crate's sources, so a changed sva-samples retires every persisted render | Non-concern: what a fingerprint keys (sva-engine) | IO: (src/**) -> SVA_SAMPLES_SRC_HASH

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
    let hash = sva_fingerprint::source_hash(std::path::Path::new("src"));
    println!("cargo:rustc-env=SVA_SAMPLES_SRC_HASH={hash:016x}");
}
