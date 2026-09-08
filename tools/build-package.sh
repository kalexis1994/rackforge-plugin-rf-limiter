#!/usr/bin/env bash
# Builds RF-Limiter and packs it with the RackForge packager, which lives in
# a sibling checkout (or wherever RACKFORGE_ROOT points).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output="${1:-$repo_root/artifacts/RF-Limiter.rfplugin}"
rackforge_root="${RACKFORGE_ROOT:-$(dirname "$repo_root")/rackforge}"

case "$output" in
  *.rfplugin) ;;
  *) echo "Plugin package output must end in .rfplugin" >&2; exit 1 ;;
esac
if [ -e "$output" ]; then
  # The packager refuses to overwrite, and so does this script: a stale
  # package that silently survives a failed build is how a "fixed" bug comes
  # back.
  echo "Refusing to overwrite existing package: $output" >&2
  exit 1
fi
if [ ! -f "$rackforge_root/Cargo.toml" ]; then
  echo "RackForge checkout not found at $rackforge_root" >&2
  exit 1
fi

cd "$repo_root"
cargo run --locked --release -p rf-limiter-lab -- metadata
# The component is built before the tests, not after: one of them loads the
# built wasm in the host's own runtime, and running it first would have it
# certify the *previous* build.
cargo build --locked --release -p rackforge-rf-limiter --target wasm32-unknown-unknown
cargo test --locked --release --workspace

mkdir -p "$(dirname "$output")"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp -R "$repo_root/plugin/package/." "$stage/"
cp "$repo_root/LICENSE" "$repo_root/NOTICE.md" "$stage/"

cargo run --manifest-path "$rackforge_root/Cargo.toml" --locked -p rackforge-store -- \
  pack-wasm "$stage" \
  "$repo_root/target/wasm32-unknown-unknown/release/rackforge_rf_limiter.wasm" \
  "$output"
echo "Packed $output"
