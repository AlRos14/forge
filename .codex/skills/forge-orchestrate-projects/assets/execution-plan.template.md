# Execution Plan — {{project_name}}

> Revision `{{revision}}` · state `{{state}}` · proposed baseline `{{baseline_id}}`

## Governing Revisions

- Charter: {{charter_revision_and_digest}}
- Product/Delivery Brief: {{artifact_revision_or_none}}
- Design: {{artifact_revision_or_none}}
- Architecture: {{artifact_revision_or_none}}
- Active decisions: {{decision_ids}}

## Work Breakdown

| Plan item | Outcome | Dependencies | Risk | Task capability | Acceptance linkage |
|---|---|---|---|---|---|
| {{item_id}} | {{outcome}} | {{dependencies}} | {{risk}} | {{capability_profile}} | {{requirement_or_check_ids}} |

## Sequencing and Parallelism

{{dependency_graph_parallel_groups_and_integration_points}}

## Validation Strategy

{{worker_checks_independent_review_manual_attestation_and_freshness}}

## Adaptive Envelope

The Project Agent may {{allowed_splitting_reordering_retry_and_substitution}} without changing {{fixed_outcome_acceptance_risk_side_effects_release_policy_or_elevated_actions}}.

## Release Policy

- Required checks: {{checks}}
- Independent review: {{review_policy}}
- Required manual attestations: {{user_attestations_or_none}}
- Waiver policy: {{user_only_and_conditions}}
- Evidence freshness and context: {{freshness_and_commit_build_context}}

## Proposed Execution Baseline

- Baseline ID/revision: {{id_revision}}
- Governing digests: {{digests}}
- Elevated or irreversible gates: {{gates_or_none}}
- Expected current baseline version: {{expected_version}}
- Approval state and user receipt: {{state_or_receipt}}
