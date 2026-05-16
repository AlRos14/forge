#!/usr/bin/env bash
set -euo pipefail

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  printf 'forge-before-work: not inside a git worktree: %s\n' "$PWD" >&2
  exit 1
fi

dirty_status="$(git status --porcelain --untracked-files=all -- . ':!FORGE_PLAN.md')"
if [[ -n "$dirty_status" ]]; then
  printf 'forge-before-work: worktree must be clean before starting work. FORGE_PLAN.md is ignored.\n' >&2
  printf 'forge-before-work: working directory: %s\n' "$(git rev-parse --show-toplevel)" >&2
  printf 'forge-before-work: dirty files:\n%s\n' "$dirty_status" >&2
  exit 1
fi
