# Forge npm bootstrapper

Run Forge from npm without installing a platform binary first:

```bash
npx @forgeailab/forge --demo
```

The package downloads the matching Forge GitHub release archive for macOS,
glibc Linux, or musl Linux, verifies it against `SHA256SUMS` when available,
caches it under `~/.forge/npx`, and starts the `forge` binary with the bundled
web UI assets.

Useful commands:

```bash
npx @forgeailab/forge --help
npx @forgeailab/forge --open
npx @forgeailab/forge ctl --help
npx -p @forgeailab/forge forge-ctl --help
```

The wrapper prints the server URL from Forge logs and does not open a browser
unless `--open` is passed. When `--open` is used, the wrapper opens the URL
from Forge's persisted server state.

By default, the wrapper downloads the GitHub release tag that matches the npm
package version. For testing a different release:

```bash
npx @forgeailab/forge --release latest
FORGE_NPX_TAG=v0.1.5 npx @forgeailab/forge
```
