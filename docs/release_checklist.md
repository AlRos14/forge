# Release Checklist

This checklist describes the release workflow in this repository. Verify live repository settings and release artifacts again before each tagged release.

At the PR14 audit on 2026-10-16, `AlRos14/forge` had no published GitHub Releases. The workspace source version is `0.8.0` and is not evidence that an installable artifact was published. GitHub private vulnerability reporting was disabled. Before publishing a release, the owner should enable it or add a private contact method to `.github/SECURITY.md`. Until this repository has a verified release channel, source builds are the supported installation path.

## Checks before tagging

- [ ] Review the changelog, migration files, license, attribution, and release notes.
- [ ] Run the applicable checks from `./scripts/ci-rust.sh` and `./scripts/ci-web.sh`; run focused functional checks for affected public surfaces.
- [ ] Confirm the intended GitHub Actions run has actual successful check results. A missing or empty check rollup is NOT RUN, not PASS.
- [ ] Build and inspect the release image/archive. Test the installer against an artifact from this repository and verify `forge --help`, `forge-ctl --help`, browser access, and checksum verification.
- [ ] Verify Docker binds to the mapped container port and persists user data in the mounted `/data` volume.
- [ ] Confirm the release tag matches the intended workspace version and inspect package-specific version metadata. Crate versions are not required to be identical.
- [ ] Verify that the tag, release, GHCR image, installer, and links resolve to `AlRos14/forge`.
- [ ] Check current repository protection, review rules, secret-scanning settings, and private vulnerability reporting; do not infer settings from this checklist.

## Workflow inventory

`.github/workflows/ci.yml` runs the `Rust`, `Security Audit`, `Web`, and `Upstream npm Bootstrapper Snapshot` jobs. The npm job syntax-checks and packs the retained upstream `npx-cli` directory; it does not publish an npm package or establish this fork's installation channel.

`.github/workflows/codeql.yml` analyzes JavaScript and TypeScript for a public repository on pushes to `main`, pull requests, and its scheduled run. `.github/workflows/scorecard.yml` publishes Scorecard results on pushes to `main` and its schedule.

A `v*` tag runs `.github/workflows/release.yml`. The build matrix creates Linux x86-64 and ARM64 GNU/musl archives and macOS x86-64 and ARM64 archives, each containing `forge`, `forge-ctl`, and `web/dist`; the workflow publishes a GitHub Release with `SHA256SUMS` and a GHCR image. The npm and Homebrew jobs are restricted to the upstream repository `ForgeAILab/forge`. They are not distribution channels for `AlRos14/forge`.

## After release

- [ ] Download one archive and verify it with `sha256sum -c SHA256SUMS`.
- [ ] Install from the matching release of this repository and test the local server and web UI.
- [ ] Pull the GHCR image and verify the exposed port, authentication setup, persistent `/data` volume, and data restart behavior.
- [ ] Check the release and container links from the README.
- [ ] Record artifact URLs, checksums, workflow run, and any platform gaps in the release notes.
