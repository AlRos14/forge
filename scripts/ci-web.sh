#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)/web"

if ! command -v pnpm >/dev/null 2>&1; then
  corepack enable
  corepack prepare pnpm@10 --activate
fi

pnpm install --frozen-lockfile
pnpm lint
pnpm typecheck
pnpm test
pnpm build
