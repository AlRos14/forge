# Forge Public Beta Release Checklist

Use this checklist for public beta releases.

## Local Verification

- [ ] Run `./scripts/ci-rust.sh`.
- [ ] Run `cargo audit`.
- [ ] Run `./scripts/ci-web.sh`.
- [ ] Run `cd web && pnpm exec playwright install --with-deps chromium && pnpm run e2e`.
- [ ] Run a repository history secret scan before making release artifacts public.
- [ ] Build a release archive locally and verify it contains `forge`, `forge-ctl`, and `web/dist/index.html`.
- [ ] Install from a release archive and confirm `forge` serves the web UI outside the repo checkout.
- [ ] Build the Docker image and confirm `/usr/local/share/forge/web/dist/index.html` exists.

## GitHub Repository Settings

- [ ] Keep `main` protected.
- [ ] Require the CI, Security Audit, CodeQL, and Scorecard checks before merge.
- [ ] Require at least one approving review.
- [ ] Require CODEOWNERS review for protected paths.
- [ ] Keep secret scanning and push protection enabled.
- [ ] Keep private vulnerability reporting enabled.

## Release Steps

- [ ] Update `CHANGELOG.md`.
- [ ] Confirm `Cargo.toml`, crate versions, and `web/package.json` versions match.
- [ ] Tag the release with `vX.Y.Z`.
- [ ] Wait for `.github/workflows/release.yml` to publish artifacts and `SHA256SUMS`.
- [ ] Download one archive, verify its checksum, install it, and smoke-test `forge --help` plus browser navigation.
- [ ] Publish release notes that call the release a public beta/developer preview.

## Post-Release

- [ ] Watch install failures, release downloads, Docker pulls, and issue response time.
- [ ] Move unresolved release blockers into the next milestone.
