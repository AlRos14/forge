use std::collections::BTreeSet;

use api_types::ActorRef as ApiActorRef;
use serde::{Deserialize, Serialize};

use crate::{
    coordinator_outputs::actor_key,
    task_coordination::{CoordinationEnvelope, WorkUnitPlan},
};

/// Scope and authority snapshot taken when the coordinator proposes a replan.
/// It is persisted in the follow-up request brief so classification never
/// measures against a later, expanded Task state.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReplanOriginalScope {
    pub objective_digest: String,
    /// Digest measured again when the returned plan is classified.
    pub current_objective_digest: String,
    pub original_envelope: CoordinationEnvelope,
    pub team_roles: BTreeSet<String>,
    pub team_actors: BTreeSet<String>,
    pub existing_work_units: usize,
    pub original_work_unit_scopes: Vec<String>,
    /// Exact immutable repository-set revision captured when this replan was proposed.
    #[serde(default)]
    pub repository_set_revision: i64,
    #[serde(default)]
    pub repository_set_digest: Option<String>,
    /// Re-read before classification and materialization; never inferred from Task.repo_id.
    #[serde(default)]
    pub current_repository_set_revision: i64,
    #[serde(default)]
    pub current_repository_set_digest: Option<String>,
    /// Legacy scalar fields remain readable for persisted pre-M3 replan briefs.
    #[serde(default)]
    pub repo_id: Option<String>,
    /// Repository identity measured again when the plan is classified.
    #[serde(default)]
    pub current_repo_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplanClassification {
    WithinEnvelope,
    ScopeOrAuthorityChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplanClassificationFacts {
    pub objective_digest_unchanged: bool,
    pub roles_within_team: bool,
    pub actors_within_team: bool,
    pub work_units_within_limit: bool,
    pub work_units_total: usize,
    pub max_work_units: usize,
    pub repository_set_unchanged: bool,
    pub repositories_within_original: bool,
    pub paths_within_original: bool,
    pub no_new_envelope_flags: bool,
    pub plan_items: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplanClassificationResult {
    pub classification: ReplanClassification,
    pub facts: ReplanClassificationFacts,
}

impl ReplanClassificationResult {
    /// Frozen JSON representation stored in `task_coordinator_replan`.
    pub fn classification_facts_json(&self) -> String {
        serde_json::to_string(&self.facts).unwrap_or_else(|_| "{}".to_owned())
    }
}

/// Classify measurable scope and authority deltas. Any false fact requires a
/// Human decision; this function never decides whether a wider plan is wise.
pub fn classify_replan(
    plan: &WorkUnitPlan,
    envelope: &CoordinationEnvelope,
    original: &ReplanOriginalScope,
) -> ReplanClassificationResult {
    let items = &plan.work_units;
    let roles_within_team = items.iter().all(|item| {
        let role = db::canonical_task_role_name(&item.role)
            .map(|role| role.to_string())
            .unwrap_or_else(|| item.role.clone());
        original.team_roles.contains(&role)
    });
    let actors_within_team = items.iter().all(|item| {
        item.assigned_actor
            .as_ref()
            .is_none_or(|actor| original.team_actors.contains(&plan_actor_key(actor)))
    });
    let work_units_total = original.existing_work_units.saturating_add(items.len());
    let original_paths = original
        .original_work_unit_scopes
        .iter()
        .flat_map(|scope| path_tokens(scope))
        .collect::<BTreeSet<_>>();
    let paths_within_original = items
        .iter()
        .flat_map(|item| path_tokens(&item.scope))
        .all(|path| path_within(&path, &original_paths));
    let repository_set_unchanged = if original.repository_set_revision == 0 {
        // Persisted pre-M3 replan snapshots only had the legacy primary ID.
        original.repo_id == original.current_repo_id
    } else {
        original.repository_set_revision == original.current_repository_set_revision
            && original.repository_set_digest == original.current_repository_set_digest
    };
    let repositories_within_original = repository_set_unchanged
        && original.current_repo_id == original.repo_id
        && items
            .iter()
            .all(|item| !item.requires_integration || original.repo_id.is_some());
    let no_new_envelope_flags = no_new_envelope_flags(envelope, &original.original_envelope);
    let facts = ReplanClassificationFacts {
        objective_digest_unchanged: original.objective_digest == original.current_objective_digest,
        roles_within_team,
        actors_within_team,
        work_units_within_limit: work_units_total <= envelope.max_work_units
            && envelope.max_work_units <= original.original_envelope.max_work_units,
        work_units_total,
        max_work_units: envelope.max_work_units,
        repository_set_unchanged,
        repositories_within_original,
        paths_within_original,
        no_new_envelope_flags,
        plan_items: items.len(),
    };
    let all_hold = facts.objective_digest_unchanged
        && facts.roles_within_team
        && facts.actors_within_team
        && facts.work_units_within_limit
        && facts.repositories_within_original
        && facts.paths_within_original
        && facts.no_new_envelope_flags;
    ReplanClassificationResult {
        classification: if all_hold {
            ReplanClassification::WithinEnvelope
        } else {
            ReplanClassification::ScopeOrAuthorityChange
        },
        facts,
    }
}

fn plan_actor_key(actor: &ApiActorRef) -> String {
    actor_key(actor)
}

fn no_new_envelope_flags(current: &CoordinationEnvelope, original: &CoordinationEnvelope) -> bool {
    !(current.allow_new_task_roles && !original.allow_new_task_roles)
        && !(current.allow_membership_changes && !original.allow_membership_changes)
        && !(current.allow_capability_changes && !original.allow_capability_changes)
        && !(current.orchestrator_reviews_findings && !original.orchestrator_reviews_findings)
        && !(current.allow_repository_write && !original.allow_repository_write)
        && !(current.allow_external_actions && !original.allow_external_actions)
        && !(current.allow_integration && !original.allow_integration)
        && !(current.allow_merge && !original.allow_merge)
        && !(!current.require_independent_review && original.require_independent_review)
        && current
            .allowed_review_roles
            .iter()
            .all(|role| original.allowed_review_roles.contains(role))
        && current
            .validation_checks
            .iter()
            .all(|pin| original.validation_checks.contains(pin))
        && current
            .fallback_agents
            .iter()
            .all(|grant| original.fallback_agents.contains(grant))
        && micro_fix_grant_within_original(current, original)
        && current.micro_fix_reviewer == original.micro_fix_reviewer
        && current.max_work_units <= original.max_work_units
        && !(current.allow_coordinator_replan && !original.allow_coordinator_replan)
}

fn micro_fix_grant_within_original(
    current: &CoordinationEnvelope,
    original: &CoordinationEnvelope,
) -> bool {
    match (&current.micro_fix, &original.micro_fix) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(current), Some(original)) => {
            (!current.enabled || original.enabled)
                && current.max_files <= original.max_files
                && current.max_changed_lines <= original.max_changed_lines
                && current.max_per_task <= original.max_per_task
                && original
                    .extra_forbidden_globs
                    .iter()
                    .all(|pattern| current.extra_forbidden_globs.contains(pattern))
        }
    }
}

fn path_tokens(scope: &str) -> Vec<String> {
    scope
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '`' | '"' | '\'' | '(' | ')'))
        .filter(|token| token.contains('/'))
        .map(|token| token.trim_start_matches("./").trim_end_matches('/'))
        .map(str::to_owned)
        .collect()
}

fn path_within(candidate: &str, originals: &BTreeSet<String>) -> bool {
    if candidate.starts_with('/') || candidate.split('/').any(|part| part == "..") {
        return false;
    }
    originals.iter().any(|original| {
        candidate == original
            || candidate
                .strip_prefix(original.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_coordination::{CoordinationEnvelope, PlanWorkUnit};

    /// Restrictive baseline so tests can measure each newly granted flag.
    fn base_envelope() -> CoordinationEnvelope {
        CoordinationEnvelope {
            max_work_units: 2,
            allow_repository_write: false,
            orchestrator_reviews_findings: false,
            ..CoordinationEnvelope::default()
        }
    }

    fn original() -> ReplanOriginalScope {
        ReplanOriginalScope {
            objective_digest: "objective".to_owned(),
            current_objective_digest: "objective".to_owned(),
            original_envelope: CoordinationEnvelope {
                max_work_units: 16,
                ..base_envelope()
            },
            team_roles: ["implementer".to_owned()].into_iter().collect(),
            team_actors: ["agent:a".to_owned()].into_iter().collect(),
            existing_work_units: 1,
            original_work_unit_scopes: vec!["src/".to_owned()],
            repository_set_revision: 1,
            repository_set_digest: Some("revision-one-digest".to_owned()),
            current_repository_set_revision: 1,
            current_repository_set_digest: Some("revision-one-digest".to_owned()),
            repo_id: Some("repo".to_owned()),
            current_repo_id: Some("repo".to_owned()),
        }
    }

    fn plan(scope: &str, role: &str, actor: Option<ApiActorRef>) -> WorkUnitPlan {
        WorkUnitPlan {
            schema_version: 1,
            request_id: "request".to_owned(),
            task_id: "task".to_owned(),
            task_version: 1,
            authorization_version: 1,
            team_digest: "team".to_owned(),
            envelope_digest: "envelope".to_owned(),
            objective_digest: "new-request-objective".to_owned(),
            work_units: vec![PlanWorkUnit {
                key: "one".to_owned(),
                title: "One".to_owned(),
                scope: scope.to_owned(),
                role: role.to_owned(),
                assigned_actor: actor,
                parent_key: None,
                depends_on: vec![],
                requires_integration: true,
                execution_instructions: None,
                review_subject_key: None,
                acceptance: vec![],
                artifact_inputs: vec![],
            }],
            delivery: None,
            review: None,
            limits: None,
        }
    }

    #[test]
    fn coordinator_replan_classifier_accepts_measured_in_envelope_plan() {
        let envelope = base_envelope();
        let result = classify_replan(
            &plan("src/module.rs", "implementer", None),
            &envelope,
            &original(),
        );
        assert_eq!(result.classification, ReplanClassification::WithinEnvelope);
        assert_eq!(
            result.classification_facts_json(),
            r#"{"objective_digest_unchanged":true,"roles_within_team":true,"actors_within_team":true,"work_units_within_limit":true,"work_units_total":2,"max_work_units":2,"repository_set_unchanged":true,"repositories_within_original":true,"paths_within_original":true,"no_new_envelope_flags":true,"plan_items":1}"#
        );
    }

    #[test]
    fn coordinator_replan_classifier_marks_scope_and_authority_deltas() {
        let mut envelope = base_envelope();
        assert_eq!(
            classify_replan(
                &plan("outside/file.rs", "implementer", None),
                &envelope,
                &original(),
            )
            .classification,
            ReplanClassification::ScopeOrAuthorityChange
        );
        assert_eq!(
            classify_replan(
                &plan(
                    "src/module.rs",
                    "new_role",
                    Some(ApiActorRef::Agent("other".into()))
                ),
                &envelope,
                &original(),
            )
            .classification,
            ReplanClassification::ScopeOrAuthorityChange
        );
        envelope.allow_repository_write = true;
        assert_eq!(
            classify_replan(
                &plan("src/module.rs", "implementer", None),
                &envelope,
                &original()
            )
            .classification,
            ReplanClassification::ScopeOrAuthorityChange
        );
    }

    #[test]
    fn coordinator_replan_classifier_requires_unchanged_objective_digest() {
        let mut original = original();
        original.current_objective_digest = "changed".to_owned();
        let envelope = base_envelope();
        let result = classify_replan(
            &plan("src/module.rs", "implementer", None),
            &envelope,
            &original,
        );
        assert_eq!(
            result.classification,
            ReplanClassification::ScopeOrAuthorityChange
        );
        assert!(!result.facts.objective_digest_unchanged);
    }

    #[test]
    fn coordinator_replan_classifier_counts_existing_work_units_and_repository_changes() {
        let mut original = original();
        original.existing_work_units = 2;
        let mut envelope = base_envelope();
        let result = classify_replan(
            &plan("src/module.rs", "implementer", None),
            &envelope,
            &original,
        );
        assert_eq!(
            result.classification,
            ReplanClassification::ScopeOrAuthorityChange
        );
        assert_eq!(result.facts.work_units_total, 3);

        original.existing_work_units = 0;
        original.current_repo_id = Some("different-repo".to_owned());
        envelope.max_work_units = 3;
        let result = classify_replan(
            &plan("src/module.rs", "implementer", None),
            &envelope,
            &original,
        );
        assert!(!result.facts.repositories_within_original);
        assert_eq!(
            result.classification,
            ReplanClassification::ScopeOrAuthorityChange
        );
    }

    #[test]
    fn multirepo_replan_classifier_detects_repository_revision_changes() {
        let mut original = original();
        original.current_repository_set_revision = 2;
        original.current_repository_set_digest = Some("revision-two-digest".to_owned());
        let result = classify_replan(
            &plan("src/module.rs", "implementer", None),
            &base_envelope(),
            &original,
        );
        assert_eq!(
            result.classification,
            ReplanClassification::ScopeOrAuthorityChange
        );
        assert!(!result.facts.repository_set_unchanged);
        assert!(!result.facts.repositories_within_original);
    }

    #[test]
    fn coordinator_replan_classifier_rejects_new_envelope_grants() {
        let mut envelope = base_envelope();
        envelope.allow_new_task_roles = true;
        assert_eq!(
            classify_replan(
                &plan("src/module.rs", "implementer", None),
                &envelope,
                &original(),
            )
            .classification,
            ReplanClassification::ScopeOrAuthorityChange
        );

        envelope.allow_new_task_roles = false;
        envelope.orchestrator_reviews_findings = true;
        assert_eq!(
            classify_replan(
                &plan("src/module.rs", "implementer", None),
                &envelope,
                &original(),
            )
            .classification,
            ReplanClassification::ScopeOrAuthorityChange
        );

        envelope.orchestrator_reviews_findings = false;
        envelope.micro_fix = Some(crate::task_coordination::EnvelopeMicroFix {
            enabled: true,
            max_files: 1,
            max_changed_lines: 1,
            max_per_task: 1,
            extra_forbidden_globs: Vec::new(),
        });
        assert_eq!(
            classify_replan(
                &plan("src/module.rs", "implementer", None),
                &envelope,
                &original(),
            )
            .classification,
            ReplanClassification::ScopeOrAuthorityChange
        );
    }
}
