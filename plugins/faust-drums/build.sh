#!/usr/bin/env bash
set -euo pipefail

crate="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd -- "$crate/../.." && pwd)"
target="$repo/target/faust-drums"
output="$repo/outputs/faust-drums"

CARGO_TARGET_DIR="$target" cargo build --manifest-path "$crate/Cargo.toml" --release --locked
mkdir -p "$output"
stage="$(mktemp "$output/.drums.XXXXXX")"
trap 'rm -f -- "$stage"' EXIT
install -m 755 "$target/release/libdaw_faust_drums.so" "$stage"
mv -f -- "$stage" "$output/West-African-Drum-Kit.clap"
install -m 644 "$crate/demo.daw.json" "$output/demo.daw.json"
printf 'Built %s\n' "$output/West-African-Drum-Kit.clap"
