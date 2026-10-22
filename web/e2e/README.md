# Playwright E2E Tests

## Prerequisites

The backend must be running on port 8080. From the repo root:

    FORGE_JWT_SECRET=test-jwt-secret-for-development cargo run -p forge-cli -- --data-dir ./test --demo

This seeds demo data and starts the API server. The suite mints admin JWTs client-side in `auth-utils.ts`, so the backend JWT secret must match; override both sides with `FORGE_E2E_JWT_SECRET` (Playwright) and `FORGE_JWT_SECRET` (backend) if you use a different value.

## Running tests

    cd web
    pnpm run e2e          # ordinary CI smoke; runs e2e/smoke.spec.ts only
    pnpm run e2e:backend  # separate local REST-to-web acceptance; requires isolated Forge
    pnpm run e2e:terminal # run the workspace terminal UI test
    pnpm run e2e:ui       # open Playwright UI mode
    pnpm run e2e:debug    # run in debug mode

The Vite dev server starts automatically via webServer config. If it is already running, Playwright will reuse it.

Most specs use `./fixtures`, which logs in a deterministic default user before navigation and attaches an auth header to Playwright API requests. Override with `FORGE_E2E_EMAIL` and `FORGE_E2E_PASSWORD` if the default account already exists with different credentials; override backend seeding with `FORGE_E2E_BACKEND_BASE_URL`. `auth.spec.ts` intentionally imports raw Playwright fixtures so it can verify logged-out flows.

`e2e` is the ordinary CI smoke and runs only `e2e/smoke.spec.ts`; it does not run the backend acceptance test. `e2e:backend` is a separate local acceptance smoke. It requires an isolated, reachable Forge backend and real authentication; it has no fallback-auth path. It creates a Project and Task through REST, verifies the same Task appears in the web board, then deletes only the Project created by that test. If `FORGE_E2E_BACKEND_BASE_URL` is overridden, the Vite API/MCP proxy follows that URL unless `VITE_API_PROXY_TARGET` is set explicitly.

## Browser install (first time)

    pnpm exec playwright install --with-deps chromium

## What the smoke tests cover

- smoke.spec.ts: application shell loads
- main-pages-render.spec.ts: target navigation and project settings render without retired surfaces
- operations.spec.ts: operator status page renders
- task-terminal.spec.ts: workspace terminal UI connects, sends input, and terminates a session

## Adding tests

Add .spec.ts files to web/e2e/. Playwright discovers them automatically.
