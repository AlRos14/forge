# Getting Started

This guide builds this repository, starts Forge locally, registers a harness
Agent, and starts one explicitly scoped WorkUnit. It uses REST so each ID and
version comes from the server response. Values such as `<PROJECT_ID>` in other
examples are placeholders; the commands below extract their own IDs.

## 1. Build and start Forge

Install Rust stable, Git, Node.js 24, pnpm 10, Bash, curl, and jq. The
`forge-cli` build script runs `pnpm install --frozen-lockfile` and builds
`web/` unless `FORGE_SKIP_WEB_BUILD=1` is set.

```bash
git clone https://github.com/AlRos14/forge.git
cd forge
corepack enable
corepack prepare pnpm@10 --activate
cargo run --locked -p forge-cli -- --data-dir ./test
```

This example uses the ignored `./test` directory. To use the normal local
installation directory instead, omit `--data-dir ./test`; Forge then uses
`~/.forge`. Do not point a real installation at a directory that you intend to
wipe. The server binds to loopback on an OS-selected port and records the
address in `<data-dir>/server.json`. Open the `management_url` printed in the
server log. Leave this terminal running.

The embedded daemon starts by default. In another terminal, from the cloned
repository, discover the saved server URL:

```bash
export FORGE_DATA_DIR="$PWD/test"
export FORGE_URL="$(jq -r '.server_url' "$FORGE_DATA_DIR/server.json")"
```

Use `~/.forge` for `FORGE_DATA_DIR` if you started Forge without the test
directory. The configuration file is `<data-dir>/forge.yaml`. Defaults are
overridden in this order: config file, environment, then supported CLI
overrides. Common settings are `FORGE_SERVER_BIND`, `FORGE_PUBLIC_BASE_URL`,
`FORGE_DATA_DIR`, `FORGE_WORKSPACE_ROOT`, `FORGE_WORKSPACE_CLEANUP_DELAY_SECONDS`,
`FORGE_JWT_SECRET`, `FORGE_WEB_DIST_DIR`, and `RUST_LOG`. The full list and
defaults are in [Configuration](architecture.md#configuration-defaults).

## 2. Register and sign in

On a new data directory, register the first local account. The first account is
the administrator used to inspect daemons and pin an Agent to a daemon. If an
account already exists, sign in as a server administrator; daemon discovery
and Agent-to-daemon pinning are admin-only. Use `/auth/login` with the same
request fields instead of `/auth/register`.

```bash
read -r -p 'Email: ' FORGE_EMAIL
read -r -s -p 'Password (at least 8 characters): ' FORGE_PASSWORD
printf '\n'

AUTH_RESPONSE="$(curl -fsS -X POST "$FORGE_URL/api/v1/auth/register" \
  -H 'Content-Type: application/json' \
  --data "$(jq -cn --arg email "$FORGE_EMAIL" --arg password "$FORGE_PASSWORD" \
    '{email:$email,password:$password}')")"
export FORGE_TOKEN="$(jq -er '.access_token' <<<"$AUTH_RESPONSE")"
unset FORGE_PASSWORD AUTH_RESPONSE

request() {
  local method="$1" path="$2"
  if [ "$#" -ge 3 ]; then
    curl -fsS -X "$method" "$FORGE_URL/api/v1$path" \
      -H "Authorization: Bearer $FORGE_TOKEN" \
      -H 'Content-Type: application/json' --data "$3"
  else
    curl -fsS -X "$method" "$FORGE_URL/api/v1$path" \
      -H "Authorization: Bearer $FORGE_TOKEN"
  fi
}
```

The examples below use `jq` to build JSON safely. REST calls require the
Bearer token. Project and Task reads follow the caller's membership; visibility
does not grant write authority.

## 3. Discover the daemon and register an Agent profile

Daemon discovery is administrator-only. Select an online host that reports an
authenticated `shell` adapter. The embedded daemon includes the shell adapter;
other harnesses appear only when installed and discovered on that host.

```bash
DAEMONS="$(request GET '/daemons')"
jq -r '.items[] | [.id, .hostname, .status,
  ([.detected_clis[] | select(.kind == "shell") | .availability] | join(","))] | @tsv' \
  <<<"$DAEMONS"
DAEMON_ID="$(jq -er '[.items[] | select(.status == "online") |
  select(any(.detected_clis[]; .kind == "shell" and .availability == "authenticated"))][0].id' \
  <<<"$DAEMONS")"

AGENT="$(request POST '/agents' "$(jq -cn --arg daemon_id "$DAEMON_ID" \
  '{name:"Local shell worker",executor_type:"shell",daemon_id:$daemon_id}')")"
AGENT_ID="$(jq -er '.id' <<<"$AGENT")"
AGENT_VERSION="$(jq -er '.version' <<<"$AGENT")"
PROFILES="$(request GET "/agents/$AGENT_ID/profiles")"
jq -r '.[] | [.id, .executor_type, (.model // "no model"), .version] | @tsv' \
  <<<"$PROFILES"
PROFILE_ID="$(jq -er '[.[] | select(.executor_type == "shell")][0].id' <<<"$PROFILES")"
request POST "/agents/$AGENT_ID/profiles/$PROFILE_ID/select" \
  "$(jq -cn --argjson version "$AGENT_VERSION" '{version:$version}')" >/dev/null
```

Agent creation supplies and selects an initial profile. The final call makes
the profile choice explicit against the Agent's returned version. For another
harness, select one actually reported in the daemon inventory and one of that
Agent's returned profiles. A remote provider or host is usable only when the
adapter supports that exact combination; unsupported combinations fail before
an Execution is admitted. Only an administrator can pin an Agent to a daemon.

The `shell` adapter runs the Execution prompt as a shell command. It is useful
for this literal smoke example and does not provide model reasoning. For
cognitive coding work, register an installed native harness such as Codex or
Claude Code and use its own selected profile and authentication. Forge does not
read the harness's private credential files.

## 4. Create a Project and register a repository

Use a local Git checkout that exists on the host where the selected daemon
runs. This example registers the Forge checkout itself; replace `REPO_PATH`
with the absolute path of the repository you intend the Agent to edit.

```bash
PROJECT="$(request POST '/projects' '{"name":"Forge getting-started example"}')"
PROJECT_ID="$(jq -er '.id' <<<"$PROJECT")"
REPO_PATH="$(git rev-parse --show-toplevel)"

REPO="$(request POST "/projects/$PROJECT_ID/repos" "$(jq -cn \
  --arg path "$REPO_PATH" \
  '{name:"main",remote_url:"",local_path:$path,default_branch:"main"}')")"
REPO_ID="$(jq -er '.id' <<<"$REPO")"
```

The path must be an existing Git repository and must be accessible at the same
absolute path from the execution host. Forge prepares the exact WorkUnit
Workspace and lease for a repository WorkUnit that requires integration. Do
not copy a Workspace path from another Task or daemon.

## 5. Create a Task, TaskRole, and RoleMembership

```bash
TASK="$(request POST "/projects/$PROJECT_ID/tasks" \
  '{"title":"Create greeting.txt"}')"
TASK_ID="$(jq -er '.id' <<<"$TASK")"

request POST "/tasks/$TASK_ID/task-roles" \
  '{"role":"implementer","coordination_mode":"independent","policy":{}}' >/dev/null
request POST "/tasks/$TASK_ID/task-roles/implementer/members" \
  "$(jq -cn --arg agent_id "$AGENT_ID" \
    '{actor_ref:{kind:"agent",id:$agent_id}}')" >/dev/null
```

TaskRole defines the responsibility. RoleMembership admits the Agent to that
role. Neither record by itself assigns executable scope.

## 6. Create the WorkUnit explicitly

The CLI currently has no WorkUnit creation command, so create it through the
implemented REST route. This repository WorkUnit names its Agent and requires
integration:

```bash
WORK_UNIT="$(request POST "/tasks/$TASK_ID/work-units" "$(jq -cn \
  --arg agent_id "$AGENT_ID" \
  '{title:"Write greeting",scope:"Create greeting.txt in the assigned repository",
    role:"implementer",parent_work_unit_id:null,
    assigned_actor:{kind:"agent",id:$agent_id},
    requires_integration:true,provenance:null}')")"
WORK_UNIT_ID="$(jq -er '.id' <<<"$WORK_UNIT")"
WORK_UNIT_VERSION="$(jq -er '.version' <<<"$WORK_UNIT")"
```

WorkUnit creation has no idempotency key. During validation of the original
PR14 head, this request returned `500 internal_error` with SQLite
`database is locked` after the WorkUnit had committed. A focused repair makes
the durable operation finish transaction acquire its SQLite write reservation
before reading. If any create response is still ambiguous, do not repeat the
`POST` blindly: inspect `GET "/tasks/$TASK_ID/work-units"`, match the intended
title, role, and Actor, and continue with the exact returned `id` and `version`
if that WorkUnit exists.

For a Task without a Repo, create a WorkUnit with
`"requires_integration":false`. It uses the stable per-Task/per-WorkUnit
context directory and has no Workspace or WorkspaceLease. These non-repository
Agent executions require the online embedded host and a supported local CLI
adapter. An unsupported remote daemon/provider combination is rejected before
the Execution enters `running`.

## 7. Start an Execution with its exact identity

This shell example writes a file. The prompt is executed by the selected shell
adapter; for a model harness, provide a natural-language task prompt instead.
The request names the WorkUnit and its current version. The Role and Agent
Actor come from that WorkUnit's allocation.

```bash
PROMPT="printf 'hello from Forge\\n' > greeting.txt"
EXECUTION="$(request POST "/tasks/$TASK_ID/executions" "$(jq -cn \
  --arg work_unit_id "$WORK_UNIT_ID" \
  --arg prompt "$PROMPT" \
  --argjson work_unit_version "$WORK_UNIT_VERSION" \
  '{work_unit_id:$work_unit_id,expected_work_unit_version:$work_unit_version,
    purpose:"implement",prompt:$prompt,input_artifact_ids:[],overrides:null}')")"
EXECUTION_ID="$(jq -er '.id' <<<"$EXECUTION")"

while :; do
  EXECUTION="$(request GET "/executions/$EXECUTION_ID")"
  STATUS="$(jq -er '.status' <<<"$EXECUTION")"
  case "$STATUS" in
    completed|failed|cancelled|stalled) break ;;
    *) sleep 1 ;;
  esac
done
if [[ "$STATUS" != "completed" ]]; then
  printf 'Execution %s ended with status %s; stop before completing or integrating the WorkUnit.\n' \
    "$EXECUTION_ID" "$STATUS" >&2
  exit 1
fi
jq '{id,status,actor_ref,role,purpose,work_unit_id,harness_session_id,workspace_id,summary}' \
  <<<"$EXECUTION"
request GET "/executions/$EXECUTION_ID/logs" | jq .
```

The Execution snapshots its Actor, role, purpose, profile/routing and any
HarnessSession. Repository executions bind the exact isolated Workspace;
non-repository executions have `workspace_id: null`. If Provider Start may
have taken effect but its result is uncertain, do not retry it automatically.
Inspect the exact Execution and follow the explicit recovery path in
[the API reference](api.md#executions-and-harnesssessions).

## 8. Review, validation, gates, integration, and closure

Execution completion does not complete its WorkUnit, advance TaskLifecycle, or
satisfy a Gate.

### 1. Confirm the exact Execution completed

Section 7 stops unless the response for `$EXECUTION_ID` is `completed`. A
failed, cancelled, stalled, or uncertain Execution cannot proceed to WorkUnit
completion or integration.

### 2. Read the current WorkUnit and version

The exact Execution in this example ran against the WorkUnit revision returned
in section 6:

```bash
WORK_UNIT="$(request GET "/work-units/$WORK_UNIT_ID")"
WORK_UNIT_VERSION="$(jq -er '.version' <<<"$WORK_UNIT")"
jq '{id,status,version,requires_integration}' <<<"$WORK_UNIT"
```

### 3. Mark that WorkUnit completed

This transition requires its completed Execution result SHA and any
dependencies to be satisfied:

```bash
COMPLETED_WORK_UNIT="$(request POST "/work-units/$WORK_UNIT_ID/status" "$(jq -cn \
  --argjson version "$WORK_UNIT_VERSION" \
  '{expected_version:$version,status:"completed"}')")"
jq '{id,status,version}' <<<"$COMPLETED_WORK_UNIT"
```

### 4. Integrate the exact Execution result

Use its exact ID and a stable key; retry this same request only with the same
key:

```bash
INTEGRATION_KEY="getting-started-$EXECUTION_ID"
INTEGRATION="$(request POST "/work-units/$WORK_UNIT_ID/integrations" "$(jq -cn \
  --arg execution_id "$EXECUTION_ID" --arg key "$INTEGRATION_KEY" \
  '{execution_id:$execution_id,idempotency_key:$key}')")"
jq '{id,outcome,source_sha,target_before_sha,target_after_sha}' <<<"$INTEGRATION"
```

### 5. Inspect the integration outcome

Only `success` proceeds through this example. `conflict`, `failed`, and
`rejected` require inspecting the exact record and an explicit recovery
decision; `running` is not terminal and must be reconciled before proceeding.
Integration does not complete other WorkUnits or close the Task.

```bash
INTEGRATION_OUTCOME="$(jq -er '.outcome' <<<"$INTEGRATION")"
case "$INTEGRATION_OUTCOME" in
  success) ;;
  conflict|failed|rejected|running)
    printf 'Integration outcome is %s; inspect the exact operation and resolve it before proceeding.\n' \
      "$INTEGRATION_OUTCOME" >&2
    exit 1
    ;;
  *) printf 'Unknown integration outcome: %s\n' "$INTEGRATION_OUTCOME" >&2; exit 1 ;;
esac
```

### 6. Read TaskLifecycle and Gates before any merge

```bash
request GET "/tasks/$TASK_ID/lifecycle" | jq .
request GET "/tasks/$TASK_ID/gates" | jq .
```

These reads are a preflight. A Gate evaluates its exact current policy and
input records; inspect the immutable GateEvaluation instead of inferring
success from a Review verdict or shell exit alone. A merge requires the exact
satisfied GateEvaluation through `POST /tasks/{id}/merge`; completing or
integrating this sample WorkUnit alone does not authorize a Task merge or
closure. A lifecycle transition that requires a Gate must carry that exact
evaluation and the current lifecycle version.

Review and deterministic validation are separate evidence paths. A Human
Review starts from the Task's Review surface or `POST /tasks/{id}/review-executions`;
the reviewer needs active membership in the Task's `reviewer` Role and must
supply the exact Ready Workspace, or `null` for a Workspace-free review.
`ValidationRun` and `Evidence` are distinct from ReviewReports. Current REST,
MCP, CLI, and web surfaces expose ValidationRuns for reading; this snapshot has
no public command to start one. Do not treat an Agent `validate` Execution as
deterministic test evidence.

For the endpoint fields, permissions, errors, and exact recovery contracts, see
[REST API](api.md), [CLI](cli.md), and the
[WorkUnit concept](concepts/work-units.md).

## Historical migration note

The V071–V076 Main/Project Agent migration narrative was moved out of this
operational guide and retained in
[historical V071–V076 notes](migration/history-v071-v076-project-os.md). It is
historical evidence, not current setup or runtime behavior.
