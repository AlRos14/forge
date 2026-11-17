# Forge Web UI

The React and TypeScript frontend for Forge. It uses Vite, TanStack Query and Router, Tailwind, and shadcn/ui, and calls the Rust API under `/api/v1`.

Routes and navigation are defined in `src/router.tsx` and `src/components/app-shell.tsx`. They include Project board, plan, and Task list; Task details and Execution history; the Project Copilot dock; Agent administration; administrator-only daemon and Operations pages; Project and server settings; and read-only historical ProjectRelease snapshots. The web UI does not currently create WorkUnits; create and allocate them through REST or MCP.

## Develop

Install the locked web dependencies and run the backend with the fixed address expected by Vite:

```bash
pnpm install --frozen-lockfile
make dev
```

In another terminal:

```bash
make frontend
```

Vite listens on `http://localhost:5173` and proxies `/api` and `/mcp` to `127.0.0.1:8080`. The Makefile sets the backend's `FORGE_SERVER_BIND` to that address by default. Override it only when you also change the Vite proxy target.

## Build and check

```bash
pnpm build
pnpm lint
pnpm typecheck
pnpm test
pnpm run e2e
```

The production build is written to `dist/`. The `forge` binary serves assets from the checkout's `web/dist/`, then from its installed `share/forge/web/dist` path. `FORGE_WEB_DIST_DIR` overrides asset discovery. The Rust build script invokes the locked pnpm install and build unless `FORGE_SKIP_WEB_BUILD=1` is set.

## Package layout

- `src/api/client.ts` — REST client.
- `src/types/generated/` — generated API bindings.
- `src/features/`, `src/components/`, and `src/pages/` — feature and page UI.
- `src/router.tsx` — TanStack Router configuration.

When a Rust API response changes, update the generated TypeScript types in the same change.
