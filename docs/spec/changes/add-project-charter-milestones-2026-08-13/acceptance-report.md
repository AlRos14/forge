# V076 acceptance report

Date: 2026-08-14
Change: `add-project-charter-milestones-2026-08-13`

## Result

The implemented Main/Project orchestration contracts, Charter-backed Project
state, Project Agent Task proposal path, Task Worker delivery/review path, and
Project evidence projection are accepted for this implementation review. A
real Smith-backed Project flow created and completed two repository Tasks, and
Forge-hosted proof media is available from the Project Overview.

This report does **not** certify the still-open full idea-to-immutable-release
scenario in task 13.5. The live run began from an existing Project's adoption
Charter, exercised the Project Agent and Task Worker portions, and deliberately
stopped before readiness/release because the test milestone has no acceptance
checks. Open checklist items remain open rather than being inferred from this
partial end-to-end run.

## Automated verification

| Gate | Result |
| --- | --- |
| `cargo fmt --all` | Passed |
| `CARGO_INCREMENTAL=0 cargo test --workspace --all-targets` | Passed; repository-documented ignored tests remain explicitly ignored |
| `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `pnpm lint` | Passed |
| `pnpm typecheck` | Passed |
| `pnpm test` | Passed: 39 files, 181 tests |
| `pnpm build` | Passed; the existing large-chunk advisory remains non-fatal |
| Strict spec validation | Passed |
| Main, Project, and cross-role skill validation | Passed |

The first full Rust attempt stopped during compilation because the shared
volume had only 31 MiB free and the accumulated `target/` tree was 126 GiB.
Only regenerable Cargo artifacts were removed with `cargo clean`; the final
Rust gates were then rerun from a clean target with incremental compilation
disabled.

## Live canonical records

| Record | Exact value |
| --- | --- |
| Project | `Todo Flow Acceptance` — `24b91a31-b110-4f2b-99a9-af8b1c095655` |
| Repository | `7b9237f1-6103-4e8c-bbcd-c8d3bb074b7a` — `test/live-todo-repo` |
| Project Agent Chat | `f6fd1481-a4f2-4469-b6c8-ecd0cfa70413` |
| Charter | `browser-todo-adoption-charter-v1`, approved revision `d4d84e45-42c0-43e9-ad40-f3c12c453b1a` (r2) |
| Charter content digest | `849a0e36b2a29259c2851ba5022174d177b9e1708b903bfed3ee2476b3481aa3` |
| Charter render digest | `1ef05977fd3b79e3a3428befce7fa91714155fe85783d793cf5fcfdeaec624dc` |
| Charter approval | `faad905f-6e96-4638-b928-ce691dbb4426`, exact event time `2026-08-14T04:39:40.323Z` |
| Milestone | `544be6d5-af57-4094-a4c2-855923758d09` — M001 `Todo CLI v0.1`, active and primary |
| Milestone definition | `ddc8ca0d-c615-4585-ae38-d5cca89cb33d`, content `fe1a48e78d3b36e0c4a64dd5ff4f57a8ced11d06443b331eabba28e4f80c2bab`, render `7850a191435e9a2ab3ee2df759ae1567e1afd88b7a33355c3cf3365213246c5f` |
| Execution baseline | `b5b05160-4ed1-43ec-81f5-d7a8d4f6a744`, revision `3665777f-0db9-4f40-86f4-643f97fca12a` |
| Baseline approval | `56150a32-39a0-48c1-a9c1-11138852a430` |
| Baseline content/render digests | `fcfe2d2d164cadb6c1832e82c534dfdc6f2dc2bf85e9256638eb4698d7ffdf8b` / `b24ac24124aabc10ea209f908adeb78e608a54c0ebb18cd2d5cec76e4fefeb09` |

## Live Project Agent and Task Worker flow

1. The Smith Project Agent produced a complete 3,582-character implementation
   proposal in turn `34c9eefa-bb10-42db-9689-6daabde1f22f`. The authenticated
   typed proposal action `ba1e1525-a613-4758-8e2b-a894b7a4ecc3` created Task
   `9b845abb-2a76-4de5-b5c0-0811dc7f1d5e`.
2. Claiming repository work with the Project Agent identity was denied. A
   separate Smith Task Worker claimed the Task. Two supervised-policy attempts
   failed visibly; after the worker profile was changed to `auto`, execution
   `acca8d65-90ac-4e2f-a049-722b08327878` completed. Independent review
   `91c4fcb0-bf98-4fc0-8f6e-fbf4f99c901b` passed 17 tests, and commit
   `485c0391a50afb994799d75b0be5670a7c677834` reached the target branch.
3. An audit found tracked Python bytecode. The Project Agent proposed a bounded
   remediation in turn `4c3ec8b7-0da0-48c9-9202-7954d5155cb0`; its success row
   has no stale retry diagnostics. Typed action
   `1eb4dd98-44f5-4ef5-8e09-4e3993dc2dca` created Task
   `34558d0e-ed17-4854-af5b-134637dc6af1` and inherited the Project default
   review command `python3 -m unittest -v`.
4. Execution `75392c25-2f53-404c-89a7-2547368c5b27` and review
   `4391e8f6-fa04-44fa-8694-505b220ecbc4` passed. A target-branch bytecode file
   produced by the independent audit initially blocked merge; removing only
   those audit-generated changes and invoking the normal recovery hook
   completed the Task. Final commit
   `2d8174481b441bfbc98fa8ed193e707db76838b2` leaves a clean repository with
   `.gitignore`, `README.md`, `todo.py`, and `test_todo.py` only.

Independent verification ran `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v`
(17 passed) and exercised `add`, pending `list`, `done 1`, completed `list`, and
JSON persistence against a temporary data file.

The run exposed and fixed four integration defects: successful Agent Chat
retries retaining stale errors, typed proposals omitting Project default review
policy, rejected orchestration identities leaving a Task branch before
workspace admission, and a racing retry/follow-up creating a second running
repository execution. Focused regression tests cover all four fixes.

## Browser and proof media

`ap-browser` exercised the signed-in Project Overview and Project Agent Chat at
1280×768, 768×1024, and 375×812. At every width
`document.documentElement.scrollWidth` equalled `clientWidth`, the page heading
and singular Project Agent navigation remained visible, and the Overview
reported 0 active, 0 blocked, and 2 terminal Tasks. Project Chat exposed all
eight authoritative record links at every width. Its enabled composer measured
848 px, 656 px, and 271 px wide respectively; at 375 px a real browser click
focused the composer and brought both it and the send control fully into the
viewport. The only recorded console error came from the installed 1Password
Chrome extension, not from Forge.

The screenshots were uploaded as Task media, reused without byte duplication
as milestone evidence, served back as `image/png` with HTTP 200, and rendered
as `3/3 available` in the Project projection.

| Width | Asset / evidence | Stable authenticated Project URL |
| --- | --- | --- |
| 1280×768 | `e5c805d5-60e1-44bb-8a25-89cc1e57be42` / `8a740b63-2988-40a7-867a-2a2a60dd3d46` | `/api/v1/projects/24b91a31-b110-4f2b-99a9-af8b1c095655/media/e5c805d5-60e1-44bb-8a25-89cc1e57be42` |
| 768×1024 | `698cbf3b-cdf6-4cb6-b2d8-8760db706dcc` / `ccf7aa87-f852-4671-badc-4abc349c0d3e` | `/api/v1/projects/24b91a31-b110-4f2b-99a9-af8b1c095655/media/698cbf3b-cdf6-4cb6-b2d8-8760db706dcc` |
| 375×812 | `8a53b734-f152-434c-965c-cc720fd21587` / `3ff32f62-a713-4006-a1a1-76c81d6caafa` | `/api/v1/projects/24b91a31-b110-4f2b-99a9-af8b1c095655/media/8a53b734-f152-434c-965c-cc720fd21587` |

Task comment `2e8a8586-12a2-4ebc-97a6-dc569ab5ef1a` contains the reviewer-facing
validation note and embedded desktop/mobile Task media references.

## Known limitations retained as open work

- A fresh Main Agent rough-idea → discovery → exact approval → Project-creation
  journey was not run live. This acceptance used an existing Project's explicit
  adoption/amendment path; the local Main binding remains setup-required.
- The Smith Project Agent Chat can write a bounded, structured proposal but
  does not yet invoke Forge typed tools from inside the CLI Chat process. The
  authenticated user/operator materialized the proposal through the typed
  action API. This is the main remaining gap for a fully autonomous Project
  Agent loop.
- No live readiness candidate, user release, release pin, or immutable
  post-release inspection was created. The milestone intentionally remains
  active and has no acceptance-check definitions.
- The complete production accessibility/performance and every-state browser
  matrix is not proven; the responsive Overview path is proven. The canonical
  legacy `happy_path` also has not yet been expanded into the full new
  Charter-to-release scenario.
- Several exhaustive race, restart, prompt-injection, failure-between-writes,
  and media-format matrices remain tracked by unchecked items in `tasks.md`.
