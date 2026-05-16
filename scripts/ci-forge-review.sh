#!/usr/bin/env bash
set -euo pipefail

./scripts/ci-rust.sh
./scripts/ci-web.sh
