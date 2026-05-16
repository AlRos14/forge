#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- --check
FORGE_SKIP_WEB_BUILD=1 cargo clippy --workspace --all-targets -- -D warnings
FORGE_SKIP_WEB_BUILD=1 cargo check --workspace --all-targets
FORGE_SKIP_WEB_BUILD=1 cargo test --workspace --all-targets
