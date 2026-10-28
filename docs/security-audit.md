# Post-V2 security audit notes

This page records dependency and code-scanning results for the post-V2
hardening pass. It is not a guarantee for future commits. Re-run the commands at
the exact candidate HEAD.

## Dependency audits

Run:

```bash
cargo audit
cd web && pnpm audit --prod
```

At the hardening HEAD:

- `cargo audit`: no known vulnerabilities. RustSec reports two non-vulnerability
  warnings:
  - `unsafe-libyaml 0.2.11` is unmaintained. It is pulled transitively by
    `serde_yaml 0.9`, which is itself deprecated. Migrating the YAML surface to a
    maintained parser is a separate compatibility change; no current advisory
    identifies an exploitable vulnerability in the paths used here.
  - `spin 0.9.8` is yanked. It is a transitive dependency of `flume` through
    `sqlx-sqlite`; the lockfile pins the exact resolved version and no advisory
    applies. Do not update it independently of SQLx without a focused test.
- `pnpm audit --prod`: clean after updating the TanStack Router family to pull
  `seroval` 1.7.2. The previous lockfile had critical/high advisories in
  `seroval` 1.5.2.
- `tokio` was updated from 1.52.3 to 1.54.1 to clear RUSTSEC-2026-0386.

## Application hardening applied

- Reusable credentials are rejected in URLs; EventSource uses a 30-second
  single-use ticket and terminal WebSockets use short-lived single-use attach
  tokens.
- JWT, PAT, OAuth MCP, refresh, daemon, and SSE-ticket authentication resolve the
  current user row. OAuth MCP tokens cannot be used as REST access tokens.
- Project authorization requires the exact membership row. Owner mutations and
  admin account deletion serialize and transfer or reject canonical ownership.
  First-user bootstrap does not claim historical ownerless Projects.
- MCP configuration responses redact credential-like query parameters, reject
  foreign/tokenized URLs as unusable, reject symlinked config files, and require
  a server administrator for server-global `user`/`local` scopes.
- Terminal WebSockets recheck project write authority every five seconds and on
  attach-token consumption.
- Agent/orchestrator/Execution admission requires the Agent owner's current
  Project membership, not owner equality alone.

## Code scanning

CodeQL analyzes JavaScript/TypeScript and Rust (`build-mode: none`, with the
web build skipped). Scorecard also publishes supply-chain findings to code
scanning. Triage at the hardening pass:

- GitHub Actions permissions were reduced to top-level `contents: read` with
  job-scoped write permissions. This addresses the `TokenPermissionsID` findings.
- The npx bootstrapper now validates that `server.json` resolves to a loopback
  HTTP(S) URL before its health request, and writes its ready marker with
  exclusive-create semantics.
- `web/scripts/lighthouse-board.mjs` remains a local developer benchmark that
  writes Lighthouse output to the ignored `test/lighthouse` directory. CodeQL's
  `js/http-to-file-access` finding describes that intentional audit artifact;
  it is not a runtime data path. Treat the directory as untrusted input if it is
  ever copied elsewhere.
- Scorecard `PinnedDependenciesID` findings are repository supply-chain
  maintenance, not application vulnerabilities. Actions and container base
  images should be pinned by digest in a separate reviewed change; Dependabot
  updates must not be applied blindly.

The CodeQL workflow and repository settings are external to a local run. The
final pull-request check rollup is the authority for the candidate HEAD; an
empty or missing rollup is NOT RUN, not PASS.

## Private vulnerability intake

GitHub private vulnerability reporting is disabled for `AlRos14/forge` as of
2026-10-28. The repository owner must enable it in
`Settings → Code security → Private vulnerability reporting`, or publish a
private contact. See [`.github/SECURITY.md`](../.github/SECURITY.md). This is a
pending external action.
