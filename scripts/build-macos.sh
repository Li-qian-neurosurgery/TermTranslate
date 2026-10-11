#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != "Darwin" ]]; then
  printf '%s\n' 'Run this script on macOS with Xcode Command Line Tools installed.' >&2
  exit 1
fi
for tool in node npm cargo rustup xcrun; do
  command -v "$tool" >/dev/null || { printf 'Missing build tool: %s\n' "$tool" >&2; exit 1; }
done
xcrun --find clang >/dev/null
# Apple Silicon only. The default ad-hoc signature requires no Apple account.
# A local Developer ID certificate can override APPLE_SIGNING_IDENTITY.
# Optional notarization: APPLE_ID, APPLE_PASSWORD (app-specific), APPLE_TEAM_ID.
export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
export CARGO_TARGET_DIR="$PWD/src-tauri/target"
rustup target add aarch64-apple-darwin
npm ci --no-audit --no-fund
npm test
cargo test --manifest-path src-tauri/Cargo.toml --locked
npm run tauri -- build --config src-tauri/tauri.macos.conf.json --target aarch64-apple-darwin -- --locked
node scripts/package-macos.mjs
