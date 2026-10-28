# Release Checklist

This checklist describes the release workflow in this repository. Verify live repository settings and release artifacts again before each tagged release.

At the PR14 audit on 2026-10-16, `AlRos14/forge` had no published GitHub Releases. The workspace source version is `0.8.0` and is not evidence that an installable artifact was published. GitHub private vulnerability reporting was disabled. Before publishing a release, the owner should enable it or add a private contact method to `.github/SECURITY.md`. Until this repository has a verified release channel, source builds are the supported installation path.

## Checks before tagging

- [ ] Review the changelog, migration files, license, attribution, and release notes.
- [ ] Run the applicable checks from `./scripts/ci-rust.sh` and `./scripts/ci-web.sh`; run focused functional checks for affected public surfaces.
- [ ] Confirm the intended GitHub Actions run has actual successful check results. A missing or empty check rollup is NOT RUN, not PASS.
- [ ] Build and inspect the release image/archive. Test the installer against an artifact from this repository and verify `forge --help`, `forge-ctl --help`, browser access, and checksum verification.
- [ ] Verify Docker runs the server as UID/GID `10001` (`forge`), persists user data in `/data` across restart, and binds the host port to loopback by default. Test both a clean volume and an explicitly authorized root-owned volume migration as described in [deployment](deployment.md).
- [ ] Confirm the release tag matches the intended workspace version and inspect package-specific version metadata. Crate versions are not required to be identical.
- [ ] Verify that the tag, release, GHCR image, installer, and links resolve to `AlRos14/forge`.
- [ ] Check current repository protection, review rules, secret-scanning settings, and private vulnerability reporting; do not infer settings from this checklist.

## Local release candidate

Do not create a tag or publish while producing a candidate. Record the exact
commit and build the artifacts locally:

```bash
git rev-parse HEAD
FORGE_SKIP_WEB_BUILD=1 cargo build --release --locked -p forge-cli -p forge-client
cd web && pnpm install --frozen-lockfile && pnpm lint && pnpm typecheck && pnpm test && pnpm build
cd ..
sha256sum target/release/forge target/release/forge-ctl web/dist/index.html
docker build -t forge-rc:$(git rev-parse --short HEAD) .
docker run -d --name forge-rc --init -p 127.0.0.1:18080:8080 \
  -v forge-rc-data:/data forge-rc:$(git rev-parse --short HEAD)
```

Verify in a second terminal:

```bash
docker exec forge-rc sh -c 'cat /proc/1/status | grep ^Uid:'
curl -fsS http://127.0.0.1:18080/healthz
docker restart forge-rc
curl -fsS http://127.0.0.1:18080/healthz
docker rm -f forge-rc
docker volume rm forge-rc-data
```

PID 1 should report `Uid: 10001 10001 10001 10001`. Confirm data persists
across restart and that the host port is loopback-only. Test an existing
root-owned volume separately with the explicit migration in
[deployment](deployment.md). Do not tag, push, or publish the candidate without
human authorization.

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
