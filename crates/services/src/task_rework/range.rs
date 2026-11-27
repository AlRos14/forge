use super::*;
use db::{
    AddWorkUnitDependency, ApplyReviewCorrections, CreateWorkUnit, ReviewCheckpointRepo,
    ReviewCorrectionLink, ReviewLedgerRepo, ReviewRangeReportRepo, ReviewReworkRound,
    RoleMembershipRepo, RoleMembershipStatus, WorkUnitRepositoryIntegrationOutcome,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct RangeReworkResult {
    pub correction_work_units: Vec<WorkUnit>,
    pub rounds_remaining: i64,
    pub replayed: bool,
}

impl TaskReworkService {
    /// Manual rework for a range ReviewReport. Corrections follow persisted
    /// line attribution; client supplied role/Actor choices are not authority.
    pub async fn request_manual_range_rework(
        &self,
        source: CollaborationActorSource,
        input: RequestReworkInput,
    ) -> Result<RangeReworkResult> {
        self.request_manual_range_rework_for_findings(source, input, None)
            .await
    }

    pub(crate) async fn request_manual_range_rework_for_findings(
        &self,
        source: CollaborationActorSource,
        input: RequestReworkInput,
        requested_finding_ids: Option<BTreeSet<String>>,
    ) -> Result<RangeReworkResult> {
        if !matches!(&source, CollaborationActorSource::Human(_)) {
            return Err(ServiceError::invalid_operation(
                "manual range rework requires a Human request",
            ));
        }
        for (field, value) in [
            ("task_id", input.task_id.as_str()),
            (
                "review_report_artifact_id",
                input.review_report_artifact_id.as_str(),
            ),
            ("correction_title", input.correction_title.as_str()),
            ("correction_scope", input.correction_scope.as_str()),
            ("idempotency_key", input.idempotency_key.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(ServiceError::invalid_operation(format!(
                    "{field} must not be empty"
                )));
            }
        }
        if input.idempotency_key.len() > 256 {
            return Err(ServiceError::invalid_operation(
                "rework idempotency_key must be at most 256 characters",
            ));
        }
        let collaboration =
            CollaborationService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let (task, actor) = collaboration
            .authorize_mutation_source(&input.task_id, &source)
            .await?;
        if task.version != input.expected_task_version {
            return Err(DbError::TaskVersionConflict {
                expected: input.expected_task_version,
                actual: task.version,
            }
            .into());
        }
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if lifecycle.version != input.expected_lifecycle_version {
            return Err(DbError::VersionConflict.into());
        }
        if !matches!(
            lifecycle.state,
            TaskLifecycleState::Ready
                | TaskLifecycleState::Active
                | TaskLifecycleState::Blocked
                | TaskLifecycleState::ReadyToMerge
        ) {
            return Err(ServiceError::invalid_operation(
                "range rework is not available in this TaskLifecycle state",
            ));
        }
        let artifact = CollaborationRepo::get_artifact(&*self.db, &input.review_report_artifact_id)
            .await?
            .filter(|artifact| {
                artifact.task_id == task.id
                    && artifact.kind == ArtifactKind::ReviewReport
                    && artifact.content.is_some()
            })
            .ok_or_else(|| {
                ServiceError::not_found(
                    "review_report_artifact",
                    input.review_report_artifact_id.clone(),
                )
            })?;
        let report = ReviewRangeReportRepo::get_review_range_report(
            &*self.db,
            &input.review_report_artifact_id,
        )
        .await?
        .filter(|report| {
            report.task_id == task.id
                && artifact.digest.as_deref() == Some(report.report_digest.as_str())
        })
        .ok_or_else(|| {
            ServiceError::invalid_operation("the report is not a current range ReviewReport")
        })?;
        if report.verdict != db::ReviewVerdict::RequestChanges {
            return Err(ServiceError::invalid_operation(
                "manual range rework requires a request_changes ReviewReport",
            ));
        }
        let existing: Option<(i64, String)> = sqlx::query_as(
            "SELECT round, idempotency_key FROM review_rework_round WHERE report_artifact_id = ?",
        )
        .bind(&report.report_artifact_id)
        .fetch_optional(self.db.pool())
        .await?;
        if let Some((_round, idempotency_key)) = existing {
            if idempotency_key != input.idempotency_key {
                return Err(DbError::IdempotencyConflict.into());
            }
            let correction_ids: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT correction_work_unit_id FROM review_correction_link
                 WHERE report_artifact_id = ? ORDER BY correction_work_unit_id",
            )
            .bind(&report.report_artifact_id)
            .fetch_all(self.db.pool())
            .await?;
            let correction_work_units = self
                .load_correction_work_units(&task.id, &correction_ids)
                .await?;
            let max_rounds: i64 = sqlx::query_scalar(
                "SELECT max_rework_rounds FROM task_autonomy_limits WHERE task_id = ?",
            )
            .bind(&task.id)
            .fetch_one(self.db.pool())
            .await?;
            let consumed =
                ReviewLedgerRepo::count_review_rework_rounds(&*self.db, &task.id).await?;
            return Ok(RangeReworkResult {
                correction_work_units,
                rounds_remaining: (max_rounds - consumed).max(0),
                replayed: true,
            });
        }

        let rows =
            ReviewLedgerRepo::load_review_coverage_rows(&*self.db, &task.id, &report.repo_id)
                .await?;
        let current_head = rows
            .integrations
            .iter()
            .filter(|integration| {
                integration.outcome == WorkUnitRepositoryIntegrationOutcome::Success
            })
            .max_by(|left, right| (&left.started_at, &left.id).cmp(&(&right.started_at, &right.id)))
            .and_then(|integration| integration.target_after_sha.clone())
            .ok_or_else(|| {
                ServiceError::invalid_operation("range rework requires a current integrated HEAD")
            })?;
        let subject = ReviewCheckpointRepo::get_work_unit_review_range(
            &*self.db,
            &report.review_work_unit_id,
        )
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation("range report has no frozen input findings")
        })?;
        let mut finding_ids = rows
            .findings
            .iter()
            .filter(|finding| finding.report_artifact_id == report.report_artifact_id)
            .map(|finding| finding.id.clone())
            .collect::<BTreeSet<_>>();
        for input_finding in &subject.input_findings {
            let resolved_by_report = rows.resolutions.iter().any(|resolution| {
                resolution.finding_id == input_finding.finding_id
                    && resolution.report_artifact_id.as_deref()
                        == Some(report.report_artifact_id.as_str())
            });
            if !resolved_by_report {
                finding_ids.insert(input_finding.finding_id.clone());
            }
        }
        if let Some(requested_finding_ids) = requested_finding_ids {
            if requested_finding_ids.is_empty() || !requested_finding_ids.is_subset(&finding_ids) {
                return Err(ServiceError::invalid_operation(
                    "manual range rework findings must be nonempty members of the exact report context",
                ));
            }
            finding_ids = requested_finding_ids;
        }
        if finding_ids.is_empty() {
            return Err(ServiceError::invalid_operation(
                "range ReviewReport has no unresolved findings to correct",
            ));
        }
        let integration_started_at = rows
            .integrations
            .iter()
            .map(|integration| (integration.id.as_str(), integration.started_at.as_str()))
            .collect::<BTreeMap<_, _>>();
        let mut authors =
            BTreeMap::<String, (Option<String>, Option<db::ActorRef>, Option<String>)>::new();
        let mut groups = BTreeMap::<(String, Option<String>), Vec<(String, String, String)>>::new();
        for finding_id in &finding_ids {
            let finding = rows
                .findings
                .iter()
                .find(|finding| finding.id == *finding_id)
                .ok_or_else(|| ServiceError::invalid_operation("range finding is missing"))?;
            let locations =
                ReviewLedgerRepo::list_review_finding_locations(&*self.db, finding_id).await?;
            let primary = locations
                .iter()
                .filter(|location| location.attribution.work_unit_id.is_some())
                .max_by(|left, right| {
                    left.attribution
                        .line_count
                        .cmp(&right.attribution.line_count)
                        .then_with(|| {
                            let left_started = left
                                .attribution
                                .integration_id
                                .as_deref()
                                .and_then(|id| integration_started_at.get(id))
                                .copied()
                                .unwrap_or("");
                            let right_started = right
                                .attribution
                                .integration_id
                                .as_deref()
                                .and_then(|id| integration_started_at.get(id))
                                .copied()
                                .unwrap_or("");
                            left_started.cmp(right_started).then_with(|| {
                                left.attribution
                                    .integration_id
                                    .cmp(&right.attribution.integration_id)
                            })
                        })
                });
            let author_work_unit_id =
                primary.and_then(|location| location.attribution.work_unit_id.clone());
            let author_actor = primary.and_then(|location| location.attribution.actor.clone());
            let author_integration_id =
                primary.and_then(|location| location.attribution.integration_id.clone());
            authors.insert(
                finding_id.clone(),
                (
                    author_work_unit_id.clone(),
                    author_actor,
                    author_integration_id,
                ),
            );
            let paths = locations
                .iter()
                .map(|location| match (location.line_start, location.line_end) {
                    (Some(start), Some(end)) => format!("{}:{start}-{end}", location.path),
                    _ => location.path.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            groups
                .entry((report.repo_id.clone(), author_work_unit_id))
                .or_default()
                .push((finding.id.clone(), finding.title.clone(), paths));
        }
        let roles = TaskRoleRepo::list_by_task(&*self.db, &task.id).await?;
        let memberships = RoleMembershipRepo::list_by_task(&*self.db, &task.id, false).await?;
        let mut implementer_roles = Vec::new();
        for role in &roles {
            let configuration =
                crate::role_capabilities::current_task_role_configuration(&self.db, &role.id)
                    .await?
                    .unwrap_or_default();
            if db::canonical_task_role_name(&role.role).as_deref() == Some("implementer")
                && !configuration
                    .capabilities
                    .contains(&api_types::TaskRoleCapability::FormalReview)
                && !configuration
                    .capabilities
                    .contains(&api_types::TaskRoleCapability::Orchestration)
            {
                implementer_roles.push(role.clone());
            }
        }
        let now = now_rfc3339();
        let mut work_units = Vec::with_capacity(groups.len());
        let mut links = Vec::new();
        let mut dependencies = Vec::new();
        let mut previous_by_repo = BTreeMap::<String, String>::new();
        let group_order = |author_id: &Option<String>| {
            author_id
                .as_deref()
                .and_then(|author_id| {
                    authors
                        .values()
                        .find(|(work_unit_id, _, _)| work_unit_id.as_deref() == Some(author_id))
                })
                .and_then(|(_, _, integration_id)| integration_id.as_deref())
                .and_then(|integration_id| {
                    integration_started_at
                        .get(integration_id)
                        .map(|started_at| ((*started_at).to_owned(), integration_id.to_owned()))
                })
        };
        let mut ordered_groups = groups.into_iter().collect::<Vec<_>>();
        ordered_groups.sort_by(|left, right| {
            let left_order = group_order(&left.0 .1);
            let right_order = group_order(&right.0 .1);
            let integration_order = match (left_order, right_order) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            };
            integration_order
                .then_with(|| left.0 .0.cmp(&right.0 .0))
                .then_with(|| left.0 .1.cmp(&right.0 .1))
        });
        for ((repo_id, author_id), findings) in ordered_groups {
            let (role_name, assigned_actor) = if let Some(author_id) = author_id.as_deref() {
                let author = WorkUnitRepo::get_by_id(&*self.db, author_id)
                    .await?
                    .filter(|unit| unit.task_id == task.id);
                if let Some(author) = author {
                    let role =
                        TaskRoleRepo::get_by_task_and_role(&*self.db, &task.id, &author.role)
                            .await?;
                    let candidates = findings
                        .iter()
                        .filter_map(|(finding_id, _, _)| authors.get(finding_id))
                        .filter(|(work_unit_id, _, _)| work_unit_id.as_deref() == Some(author_id))
                        .map(|(_, actor, _)| actor.clone())
                        .collect::<Vec<_>>();
                    let candidate = match candidates.first() {
                        Some(Some(first))
                            if candidates
                                .iter()
                                .all(|candidate| candidate.as_ref() == Some(first)) =>
                        {
                            Some(first.clone())
                        }
                        _ => None,
                    };
                    let eligible = role.as_ref().is_some_and(|role| {
                        candidate.as_ref().is_some_and(|actor| {
                            memberships.iter().any(|(membership_role, membership)| {
                                membership_role.id == role.id
                                    && membership.status == RoleMembershipStatus::Active
                                    && membership.actor_ref() == *actor
                            })
                        })
                    });
                    if eligible {
                        (author.role, candidate)
                    } else {
                        fallback_implementer(&implementer_roles)?
                    }
                } else {
                    fallback_implementer(&implementer_roles)?
                }
            } else {
                fallback_implementer(&implementer_roles)?
            };
            let id = new_uuid_v4();
            let finding_list = findings
                .iter()
                .map(|(finding_id, title, _)| format!("{finding_id}: {title}"))
                .collect::<Vec<_>>()
                .join("; ");
            let correction_scope = format!(
                "{}\n\nCorrect exact range findings {finding_list} in repository {repo_id}. Start from the current integrated HEAD and leave the original WorkUnit unchanged.",
                input.correction_scope
            );
            let paths = findings
                .iter()
                .map(|(finding_id, _, paths)| format!("{finding_id}: {paths}"))
                .collect::<Vec<_>>()
                .join("\n");
            work_units.push(CreateWorkUnit {
                id: id.clone(),
                task_id: task.id.clone(),
                parent_work_unit_id: None,
                title: format!("{}: {}", input.correction_title, findings[0].1),
                scope: correction_scope,
                execution_instructions: Some(format!(
                    "{paths}\nSource ReviewReport: {}",
                    report.report_artifact_id
                )),
                role: role_name,
                assigned_actor,
                requires_integration: true,
                provenance: Some(db::WorkUnitProvenance::Artifact(
                    report.report_artifact_id.clone(),
                )),
                artifact_input_ids: vec![report.report_artifact_id.clone()],
                repository_ids: vec![repo_id.clone()],
                review_approval_gate_id: None,
                review_approval_line_id: None,
                created_by: actor.clone(),
                created_at: now.clone(),
            });
            if let Some(previous) = previous_by_repo.insert(repo_id.clone(), id.clone()) {
                dependencies.push(AddWorkUnitDependency {
                    work_unit_id: id.clone(),
                    depends_on_work_unit_id: previous,
                    expected_version: 1,
                    created_by: actor.clone(),
                    created_at: now.clone(),
                });
            }
            for (finding_id, _, _) in findings {
                links.push(ReviewCorrectionLink {
                    correction_work_unit_id: id.clone(),
                    finding_id,
                    task_id: task.id.clone(),
                    repo_id: repo_id.clone(),
                    report_artifact_id: report.report_artifact_id.clone(),
                    base_commit: current_head.clone(),
                    created_at: now.clone(),
                });
            }
        }
        let consumed: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM task_rework_receipt WHERE task_id = ?)
                  + (SELECT COUNT(*) FROM review_rework_round WHERE task_id = ?)
                  + (SELECT COUNT(*) FROM task_review_triage triage WHERE triage.task_id = ?
                       AND (triage.disposition = 'recheck' OR EXISTS (
                           SELECT 1 FROM review_finding_triage finding_triage
                           WHERE finding_triage.report_artifact_id = triage.review_report_artifact_id
                             AND finding_triage.disposition = 'recheck'
                       )))",
        )
        .bind(&task.id)
        .bind(&task.id)
        .bind(&task.id)
        .fetch_one(self.db.pool())
        .await?;
        let max_rounds: i64 = sqlx::query_scalar(
            "SELECT max_rework_rounds FROM task_autonomy_limits WHERE task_id = ?",
        )
        .bind(&task.id)
        .fetch_one(self.db.pool())
        .await?;
        if consumed >= max_rounds {
            return Err(ServiceError::conflict(
                "range correction and recheck round budget is exhausted",
            ));
        }
        let round = ReviewReworkRound {
            report_artifact_id: report.report_artifact_id.clone(),
            task_id: task.id.clone(),
            round: consumed + 1,
            request_id: None,
            created_by: actor.clone(),
            idempotency_key: input.idempotency_key,
            created_at: now.clone(),
        };
        let mut events = work_units
            .iter()
            .map(|unit| CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "work_unit.created".to_owned(),
                entity_type: "work_unit".to_owned(),
                entity_id: unit.id.clone(),
                actor_type: actor.kind().to_string(),
                actor_id: Some(actor.id().to_owned()),
                scope_type: "task".to_owned(),
                scope_id: task.id.clone(),
                correlation_id: report.report_artifact_id.clone(),
                causation_id: Some(report.execution_id.clone()),
                causation_depth: 1,
                dedupe_key: Some(format!("work_unit.created:{}", unit.id)),
                payload_json: serde_json::json!({
                    "task_id": task.id,
                    "work_unit_id": unit.id,
                    "rework_round": round.round,
                    "rework_kind": "range_review",
                    "review_report_artifact_id": report.report_artifact_id,
                })
                .to_string(),
                created_at: now.clone(),
            })
            .collect::<Vec<_>>();
        events.push(CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "task.rework_requested".to_owned(),
            entity_type: "task".to_owned(),
            entity_id: task.id.clone(),
            actor_type: actor.kind().to_string(),
            actor_id: Some(actor.id().to_owned()),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: report.report_artifact_id.clone(),
            causation_id: Some(report.execution_id.clone()),
            causation_depth: 1,
            dedupe_key: Some(format!(
                "task.rework_requested:range-review:{}",
                report.report_artifact_id
            )),
            payload_json: serde_json::json!({
                "task_id": task.id,
                "review_report_artifact_id": report.report_artifact_id,
                "correction_work_unit_ids": work_units.iter().map(|unit| unit.id.clone()).collect::<Vec<_>>(),
                "round": round.round,
            })
            .to_string(),
            created_at: now.clone(),
        });
        let event_dedupe_keys = events
            .iter()
            .map(|event| event.dedupe_key.clone().unwrap_or_else(|| event.id.clone()))
            .collect::<Vec<_>>();
        db::ReviewLedgerRepo::apply_review_corrections(
            &*self.db,
            ApplyReviewCorrections {
                round: round.clone(),
                work_units,
                dependencies,
                links,
                max_rework_rounds: max_rounds,
                events,
            },
        )
        .await?;
        for dedupe_key in event_dedupe_keys {
            if let Some(event) =
                DomainEventRepo::get_event_by_dedupe(&*self.db, &dedupe_key).await?
            {
                crate::domain_event_service::DomainEventService::publish_committed_hint(
                    &self.event_bus,
                    &event,
                );
            }
        }
        let correction_ids: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT correction_work_unit_id FROM review_correction_link
             WHERE report_artifact_id = ? ORDER BY correction_work_unit_id",
        )
        .bind(&report.report_artifact_id)
        .fetch_all(self.db.pool())
        .await?;
        let consumed = ReviewLedgerRepo::count_review_rework_rounds(&*self.db, &task.id).await?;
        Ok(RangeReworkResult {
            correction_work_units: self
                .load_correction_work_units(&task.id, &correction_ids)
                .await?,
            rounds_remaining: (max_rounds - consumed).max(0),
            replayed: false,
        })
    }

    async fn load_correction_work_units(
        &self,
        task_id: &str,
        ids: &[String],
    ) -> Result<Vec<WorkUnit>> {
        let mut units = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(unit) = WorkUnitRepo::get_by_id(&*self.db, id)
                .await?
                .filter(|unit| unit.task_id == task_id)
            {
                units.push(unit);
            }
        }
        Ok(units)
    }
}

fn fallback_implementer(roles: &[db::TaskRole]) -> Result<(String, Option<db::ActorRef>)> {
    let mut roles = roles.iter();
    let Some(role) = roles.next() else {
        return Err(ServiceError::Coordination {
            code: "review.finding.rework_requires_human",
            message: "no implementer TaskRole can own this unattributed correction".to_owned(),
        });
    };
    if roles.next().is_some() {
        return Err(ServiceError::Coordination {
            code: "review.finding.rework_requires_human",
            message: "multiple implementer TaskRoles can own this unattributed correction"
                .to_owned(),
        });
    }
    Ok((role.role.clone(), None))
}
