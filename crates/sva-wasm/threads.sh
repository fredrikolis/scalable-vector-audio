#!/usr/bin/env bash
# Concern: builds the browser bundle with wasm threads on the pinned toolchain | Non-concern: publishing it (release.yml) | IO: (out dir, wasm-pack args) -> a package
set -euo pipefail
out="${1:?usage: threads.sh <out dir, from this crate> [wasm-pack build args]}"
shift
# std rebuilt with atomics (-Z build-std, which pinned stable takes under RUSTC_BOOTSTRAP); simd128 again, as RUSTFLAGS replaces .cargo/config.toml's.
export RUSTC_BOOTSTRAP=1
RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals,+simd128"
for arg in --shared-memory --import-memory --max-memory=4294967296 \
  --export=__wasm_init_tls --export=__tls_size --export=__tls_align --export=__tls_base; do
  RUSTFLAGS+=" -C link-arg=$arg"
done
export RUSTFLAGS
cd "$(dirname "$0")"
wasm-pack build --release --target web --out-dir "$out" "$@" . -- -Z build-std=std,panic_abort
if [ -d "$out/snippets" ]; then
  jq '.files += ["snippets"] | .files |= unique' "$out/package.json" > "$out/package.json.tmp"
  mv "$out/package.json.tmp" "$out/package.json"
fi
