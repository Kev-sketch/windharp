#!/usr/bin/env bash
# Build a release artifact with personal filesystem paths stripped.
#
# Rust bakes absolute source paths into binaries as panic-location metadata, so a
# plain `cargo build` embeds the builder's home directory (/Users/<name>/...).
# This wraps the tauri build with --remap-path-prefix so shipped binaries contain
# no personal paths.
#
# Usage: scripts/build-release.sh <rust-target> [extra tauri args...]
#   scripts/build-release.sh aarch64-apple-darwin
#   scripts/build-release.sh x86_64-pc-windows-msvc
set -euo pipefail

TARGET="${1:?usage: build-release.sh <rust-target> [extra args]}"
shift || true

# Remap the builder's home to a neutral relative marker. Applies to both the
# cargo registry (~/.cargo) and the project checkout, which live under $HOME.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=."

echo "building target=$TARGET with RUSTFLAGS=$RUSTFLAGS"
exec pnpm exec tauri build --target "$TARGET" "$@"
