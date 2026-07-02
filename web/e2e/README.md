# Playwright E2E Tests

## Prerequisites

The backend must be running on port 8080. From the repo root:

    FORGE_JWT_SECRET=test-jwt-secret-for-development cargo run -p forge-cli -- --data-dir ./test --demo

This seeds demo data and starts the API server. The suite mints admin JWTs client-side in `auth-utils.ts`, so the backend JWT secret must match; override both sides with `FORGE_E2E_JWT_SECRET` (Playwright) and `FORGE_JWT_SECRET` (backend) if you use a different value.

## Running tests

    cd web
    pnpm run e2e          # run all tests (headless)
    pnpm run e2e:ui       # open Playwright UI mode
    pnpm run e2e:debug    # run in debug mode

The Vite dev server starts automatically via webServer config. If it is already running, Playwright will reuse it.

Most specs use `./fixtures`, which logs in a deterministic default user before navigation and attaches an auth header to Playwright API requests. Override with `FORGE_E2E_EMAIL` and `FORGE_E2E_PASSWORD` if the default account already exists with different credentials; override backend seeding with `FORGE_E2E_BACKEND_BASE_URL`. `auth.spec.ts` intentionally imports raw Playwright fixtures so it can verify logged-out flows.

## Browser install (first time)

    pnpm exec playwright install --with-deps chromium

## What the smoke tests cover

- smoke.spec.ts: page loads, navigation visible
- kanban-board.spec.ts: board columns render correctly
- task-transition-timeline.spec.ts: task detail transition timeline renders
- task-media.spec.ts: task comments upload and preview real image/video media fixtures

## Adding tests

Add .spec.ts files to web/e2e/. Playwright discovers them automatically.
