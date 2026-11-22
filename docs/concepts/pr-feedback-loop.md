# PR feedback loop

Forge links remote pull request feedback to the Task and exact repository
admission that produced the PR. This is an evidence and rework path; it does not
create a merge action.

Each changed repository has its own PR metadata and admission. Task PR feedback,
external-action, and rework responses carry the exact `repo_id` and
`admission_id`, so a Task with several PRs never infers repository identity from
a scalar Task field. Human action approval remains bound to that exact
admission, and merging each PR remains a Human operation.

## Connect GitHub and configure a repository

A Project owner or admin connects a GitHub SCM credential through POST
/providers. The request carries provider=github, a label, and the credential in
its body. The credential is accepted for connection, encrypted by
CredentialService, and never returned in provider responses. Authenticate API
requests with an Authorization: Bearer <access-token> header. Do not place
credentials in a URL, shell history, or source example.

GET /providers?purpose=scm lists the caller's SCM credential handles. The
ordinary provider list excludes SCM handles. Configure the repository's PR
provider as GitHub and reference the selected handle with
pr_provider_config.credential_id when creating or updating the Repo. Forge
checks current Project owner/admin authority and that the caller owns the
credential. Repo projections expose has_credential, credential_handle_id, and
credential_label, never the secret.

Use GET /repos/{id}/pr-feedback-policy to read the policy and updated_at, then
PUT the new policy with expected_updated_at. This compare-and-set prevents
stale settings from overwriting a concurrent update. Policy changes affect new
remote PR admissions; an existing admission keeps its frozen credential and
policy snapshot.

| Policy field | Meaning and default |
| --- | --- |
| schema_version | Currently 1. |
| ingest | Whether Forge ingests external feedback. Default false. |
| trusted_author_associations | GitHub author associations admitted as trusted. Default OWNER, MEMBER, and COLLABORATOR. |
| trusted_logins | Additional trusted GitHub logins. Default empty. |
| max_items_per_poll | Maximum feedback items read per poll. Default 50; valid range is 1 to 100. |
| push_corrections | Authorization for pushing a correction to the admitted PR. Default human_approval; automatic requires explicit owner/admin policy. |
| post_summary | Whether to post a deterministic triage summary. Default off; human_approval and automatic are available policy values. |

The per-Repo policy is an explicit Project owner/admin decision. Turning on
ingestion does not make every provider comment trusted. Changing a policy does
not retroactively widen an admission already created.

## Ingest and triage feedback

The reconciler stores reviews, inline comments, and issue comments as durable
feedback items with author association, trust class, body digest, head state,
and triage disposition. Provider-authored text is untrusted input. Only trusted,
current-head items may enter coordinator context; untrusted or stale-head items
remain inspectable to Task readers and do not start a coordinator turn. Public
SSE events carry allowlisted identifiers, digests, heads, kinds, and states,
never feedback bodies or credentials.

An authorized coordinator receives a bounded set tied to one admission and
chooses one disposition for each item:

- extend_task requests an in-scope correction in the existing PR. Forge
  supersedes the active publication admission without touching the remote PR,
  resumes the Task, and materializes exact WorkUnits from the typed plan.
- propose_task asks a Human to authorize separate follow-up scope. Approving
  that exact decision creates a dependent follow-up Task; the current PR is
  unchanged.
- human records a Human decision for the item.
- dismiss stores the disposition and reason without creating correction work.

Scope classification is deterministic. A proposed extension outside the
authorized Task scope is rejected. A changed remote head, external merge, stale
request, or out-of-scope item follows its explicit conflict or Human decision
path. Any rework proceeds through normal WorkUnit execution, formal review,
Gate evaluation, integration, and PR admission.

## External PR actions

Forge supports two PR actions: push_update and post_comment. A push update uses
the admitted result and checks the remote head before writing. A summary comment
uses a deterministic summary Artifact and its anti-loop marker. Each action is
a durable, versioned record with its own state and expected remote-head facts.
An uncertain result is reconciled against provider state before any retry.

By default, push_update is proposed for Human approval and post_comment is off.
Project owners/admins can set push_corrections=automatic or
post_summary=automatic in the frozen repository policy. Human approval or
rejection is available only to an authorized Project owner/admin and is tied to
the exact action version. Forge exposes no merge action; merging the PR remains
a Human operation.

## Public surfaces

Task-scoped reads and Human action decisions use these REST routes (the API
reference omits the common /api/v1 prefix):

- GET /tasks/{id}/pr-feedback
- GET /tasks/{id}/pr-rework
- GET /tasks/{id}/pr-actions
- POST /tasks/{id}/pr-actions/{action_id}/approve
- POST /tasks/{id}/pr-actions/{action_id}/reject
- GET and PUT /repos/{id}/pr-feedback-policy

The read-only MCP tools are forge_list_task_pr_feedback,
forge_list_task_pr_rework, and forge_list_task_pr_actions. No MCP tool answers
Human decisions or publishes an action.

Public event names are task.pr_feedback_ingested,
task.pr_feedback_triaged, task.pr_rework_superseded,
task.pr_rework_resumed, task.pr_rework_converged, task.pr_followup_proposed,
task.pr_action_requested, and task.pr_action_completed. Event payloads omit
provider-authored bodies, secrets, and free-form instructions.

See the [REST, MCP, and SSE reference](../api.md), [orchestration](orchestration.md),
and [deployment secret guidance](../deployment.md).
