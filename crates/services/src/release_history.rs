//! Read-only access to immutable ProjectRelease history.
//!
//! The archive tables are populated from the exact V120 rows before PR13
//! removes the Project OS schema. This reader deliberately has no write path
//! and does not consult current milestone, readiness, baseline, or Charter
//! state.

use std::sync::Arc;

use api_types::{
    canonical_digest_with_schema, AuthorizationProvenance, PrincipalKind, PrincipalRef,
    ProjectRelease, ReleaseSnapshot,
};
use chrono::DateTime;
use db::{ProjectReleaseRecord, SqliteDb};
use serde::de::DeserializeOwned;
use serde_json::json;
use sqlx::{sqlite::SqliteRow, Row};

const MAX_AUTHORIZATION_TIMESTAMP_LEN: usize = 64;
const RELEASE_SCHEMA_VERSION: &str = "forge.milestone-release/v1";
const RELEASE_DIGEST_SCHEMA_VERSION: &str = "forge.milestone-release/v1";

#[derive(Debug, Clone)]
pub struct HistoricalReleaseReader {
    db: Arc<SqliteDb>,
}

impl HistoricalReleaseReader {
    #[must_use]
    pub fn new(db: Arc<SqliteDb>) -> Self {
        Self { db }
    }

    pub async fn get_release(
        &self,
        project_id: &str,
        release_id: &str,
    ) -> crate::Result<Option<ProjectRelease>> {
        let row = sqlx::query("SELECT * FROM project_release WHERE id = ? AND project_id = ?")
            .bind(release_id)
            .bind(project_id)
            .fetch_optional(self.db.pool())
            .await?;
        match row {
            Some(row) => Ok(Some(
                self.release_from_record(release_record_from_row(row)?)
                    .await?,
            )),
            None => Ok(None),
        }
    }

    pub async fn list_releases(
        &self,
        project_id: &str,
        milestone_id: &str,
    ) -> crate::Result<Vec<ProjectRelease>> {
        let rows = sqlx::query(
            "SELECT * FROM project_release
             WHERE project_id = ? AND milestone_id = ?
             ORDER BY release_revision ASC, id ASC",
        )
        .bind(project_id)
        .bind(milestone_id)
        .fetch_all(self.db.pool())
        .await?;
        let mut releases = Vec::with_capacity(rows.len());
        for row in rows {
            releases.push(
                self.release_from_record(release_record_from_row(row)?)
                    .await?,
            );
        }
        Ok(releases)
    }

    async fn release_from_record(
        &self,
        record: ProjectReleaseRecord,
    ) -> crate::Result<ProjectRelease> {
        if record.schema_version != RELEASE_SCHEMA_VERSION
            || record.snapshot_digest.trim().is_empty()
            || record.release_revision <= 0
            || record.release_sequence <= 0
        {
            return Err(invalid(
                "immutable release record has invalid schema or identity fields",
            ));
        }

        let milestone = sqlx::query(
            "SELECT milestone_id, project_id, milestone_key, display_label
             FROM project_release_history_milestone WHERE release_id = ?",
        )
        .bind(&record.id)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| {
            invalid(format!(
                "release {} has no frozen milestone identity",
                record.id
            ))
        })?;
        if milestone.try_get::<String, _>("milestone_id")? != record.milestone_id
            || milestone.try_get::<String, _>("project_id")? != record.project_id
        {
            return Err(invalid(
                "immutable release milestone identity does not match its snapshot",
            ));
        }
        let milestone_canonical_id: String = milestone.try_get("milestone_key")?;
        let display_label: Option<String> = milestone.try_get("display_label")?;

        let definition = sqlx::query(
            "SELECT id, milestone_id, content_digest
             FROM project_release_history_definition
             WHERE release_id = ? AND id = ?",
        )
        .bind(&record.id)
        .bind(&record.milestone_revision_id)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| invalid("release definition snapshot is missing"))?;
        if definition.try_get::<String, _>("milestone_id")? != record.milestone_id {
            return Err(invalid(
                "immutable release definition belongs to another milestone",
            ));
        }
        let definition_digest: String = definition.try_get("content_digest")?;
        if definition_digest.trim().is_empty() {
            return Err(invalid("immutable release definition digest is empty"));
        }

        let readiness = sqlx::query(
            "SELECT id, project_id, milestone_id, expected_milestone_version,
                    event_watermark, readiness_digest, baseline_id,
                    baseline_revision_id, baseline_digest, release_policy_revision,
                    release_policy_digest
             FROM project_release_history_readiness
             WHERE release_id = ? AND id = ?",
        )
        .bind(&record.id)
        .bind(&record.readiness_snapshot_id)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| invalid("release readiness snapshot is missing"))?;
        if readiness.try_get::<String, _>("project_id")? != record.project_id
            || readiness.try_get::<String, _>("milestone_id")? != record.milestone_id
            || readiness.try_get::<String, _>("readiness_digest")? != record.readiness_digest
            || readiness.try_get::<String, _>("baseline_id")? != record.baseline_id
            || readiness.try_get::<String, _>("baseline_revision_id")?
                != record.baseline_revision_id
            || readiness.try_get::<String, _>("baseline_digest")? != record.baseline_digest
            || readiness.try_get::<String, _>("release_policy_revision")?
                != record.release_policy_revision
            || readiness.try_get::<String, _>("release_policy_digest")?
                != record.release_policy_digest
        {
            return Err(invalid(
                "immutable release references disagree with readiness history",
            ));
        }
        let expected_milestone_version: i64 = readiness.try_get("expected_milestone_version")?;
        let source_event_watermark: String = readiness.try_get("event_watermark")?;
        if expected_milestone_version <= 0 || source_event_watermark.trim().is_empty() {
            return Err(invalid(
                "release readiness history is missing identity or watermark",
            ));
        }

        let charter_revision_id = record
            .charter_revision_id
            .as_deref()
            .ok_or_else(|| invalid("immutable release is missing its Charter revision"))?;
        let charter = sqlx::query(
            "SELECT charter_id, revision_id, content_digest, render_version,
                    rendered_digest
             FROM project_release_history_charter
             WHERE release_id = ? AND revision_id = ?",
        )
        .bind(&record.id)
        .bind(charter_revision_id)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| invalid("release Charter ArtifactRef snapshot is missing"))?;
        let charter_revision = api_types::ArtifactRef {
            artifact_id: charter.try_get("charter_id")?,
            revision_id: charter.try_get("revision_id")?,
            content_digest: charter.try_get("content_digest")?,
            render_version: Some(charter.try_get("render_version")?),
            render_digest: Some(charter.try_get("rendered_digest")?),
        };

        let (frozen_evidence_pins, projected_evidence_pins) = self
            .release_pins(
                &record.project_id,
                &record.id,
                &record.evidence_references_json,
            )
            .await?;
        let released_by = PrincipalRef {
            kind: principal_kind(&record.releasing_principal_type)?,
            id: record.releasing_principal_id.clone(),
            display_name: None,
        };
        let authorization = AuthorizationProvenance {
            principal: released_by.clone(),
            authorization_basis: record.authorization_basis.clone(),
            action: record.authorization_action.clone(),
            event_id: record.explicit_event.clone(),
            occurred_at: record.authorization_occurred_at.clone(),
        };
        validate_persisted_authorization(&authorization, "project.milestone.release")?;

        let mut snapshot = ReleaseSnapshot {
            schema_version: record.schema_version.clone(),
            project_id: record.project_id.clone(),
            milestone_id: record.milestone_id.clone(),
            milestone_canonical_id,
            release_revision: record.release_revision,
            release_identity: record.release_identifier.clone(),
            milestone_definition_revision_id: record.milestone_revision_id.clone(),
            milestone_definition_digest: definition_digest,
            expected_milestone_version,
            display_label,
            summary: record.summary.clone(),
            changelog: parse_json_required(&record.changelog, "release changelog")?,
            known_issues: parse_json_required(&record.known_issues_json, "release known_issues")?,
            readiness_snapshot_id: record.readiness_snapshot_id.clone(),
            readiness_digest: record.readiness_digest.clone(),
            source_event_watermark,
            baseline_id: record.baseline_id.clone(),
            baseline_revision_id: record.baseline_revision_id.clone(),
            baseline_digest: record.baseline_digest.clone(),
            charter_revision,
            document_revisions: parse_json_required(
                &record.document_revisions_json,
                "release document_revisions",
            )?,
            included_decisions: parse_json_required(
                &record.decision_ids_json,
                "release decision_ids",
            )?,
            included_tasks: parse_json_required(
                &record.task_references_json,
                "release task_references",
            )?,
            validation_results: parse_json_required(
                &record.validation_references_json,
                "release validation_references",
            )?,
            repository_references: parse_json_required(
                &record.git_references_json,
                "release git_references",
            )?,
            evidence_pins: frozen_evidence_pins,
            waived_check_ids: parse_json_required(&record.waivers_json, "release waivers")?,
            release_policy_revision: record.release_policy_revision.clone(),
            release_policy_digest: record.release_policy_digest.clone(),
            released_by,
            authorization,
            released_at: record.created_at.clone(),
            idempotency_key: record.idempotency_key.clone(),
            snapshot_digest: record.snapshot_digest.clone(),
        };

        let digest = release_snapshot_digest(&snapshot)
            .map_err(|error| invalid(format!("invalid immutable Release snapshot: {error}")))?;
        if digest != record.snapshot_digest {
            return Err(invalid(
                "immutable release snapshot digest does not match its contents",
            ));
        }
        self.validate_release_references(&record.id, &snapshot)
            .await?;
        // Availability is a read-time overlay from immutable tombstones. Keep
        // the stored pin projection in the digest input, then expose the
        // current audited projection after the frozen snapshot has verified.
        snapshot.evidence_pins = projected_evidence_pins;
        Ok(ProjectRelease {
            id: record.id,
            project_id: record.project_id,
            milestone_id: record.milestone_id,
            release_sequence: record.release_sequence,
            release_identity: record.release_identifier,
            snapshot,
            version: record.release_revision,
            created_at: record.created_at,
        })
    }

    async fn validate_release_references(
        &self,
        release_id: &str,
        snapshot: &ReleaseSnapshot,
    ) -> crate::Result<()> {
        type ExpectedReference = (
            &'static str,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
        );
        let mut expected = Vec::<ExpectedReference>::new();
        for task in &snapshot.included_tasks {
            expected.push((
                "task",
                task.task_id.clone(),
                Some(task.task_version.to_string()),
                Some(task.task_state.clone()),
                None,
                serde_json::to_string(task).map_err(json_error)?,
            ));
        }
        for validation in &snapshot.validation_results {
            expected.push((
                "validation",
                validation.validation_id.clone(),
                Some(validation.evaluated_at.clone()),
                Some(format!("{:?}", validation.status).to_ascii_lowercase()),
                Some(validation.result_digest.clone()),
                serde_json::to_string(validation).map_err(json_error)?,
            ));
        }
        for document in &snapshot.document_revisions {
            expected.push((
                "document",
                document.revision_id.clone(),
                None,
                Some("approved".to_owned()),
                Some(document.content_digest.clone()),
                serde_json::to_string(document).map_err(json_error)?,
            ));
        }
        for decision in &snapshot.included_decisions {
            expected.push((
                "decision",
                decision.decision_id.clone(),
                None,
                Some(format!("{:?}", decision.state).to_ascii_lowercase()),
                Some(decision.digest.clone()),
                serde_json::to_string(decision).map_err(json_error)?,
            ));
        }
        for repository in &snapshot.repository_references {
            let reference: RepositoryContextReference =
                parse_json_required(repository, "release repository reference")?;
            validate_repository_context_reference(&reference)?;
            let record_id = reference
                .execution_id
                .as_deref()
                .unwrap_or(reference.task_id.as_str())
                .to_owned();
            let record_digest =
                canonical_digest_with_schema(RELEASE_DIGEST_SCHEMA_VERSION, &reference)
                    .map_err(json_error)?;
            expected.push((
                "git_ref",
                record_id,
                Some(reference.task_version.to_string()),
                reference.execution_status.clone(),
                Some(record_digest),
                serde_json::to_string(&reference).map_err(json_error)?,
            ));
            if let Some(review_id) = reference.review_id.as_deref() {
                let review_version = reference
                    .review_updated_at
                    .clone()
                    .or_else(|| reference.review_created_at.clone());
                let review_digest = canonical_digest_with_schema(
                    RELEASE_DIGEST_SCHEMA_VERSION,
                    &json!({
                        "review_id": review_id,
                        "status": reference.review_status.clone(),
                        "ci_results": reference.ci_results.clone(),
                        "audit_result": reference.audit_result.clone(),
                        "human_decision": reference.human_decision.clone(),
                        "created_at": reference.review_created_at.clone(),
                        "updated_at": reference.review_updated_at.clone(),
                    }),
                )
                .map_err(json_error)?;
                expected.push((
                    "review",
                    review_id.to_owned(),
                    review_version,
                    reference.review_status.clone(),
                    Some(review_digest),
                    serde_json::to_string(&reference).map_err(json_error)?,
                ));
            }
        }
        for issue in &snapshot.known_issues {
            expected.push((
                "known_issue",
                issue.clone(),
                None,
                None,
                None,
                "{}".to_owned(),
            ));
        }
        let rows = sqlx::query(
            "SELECT reference_kind, record_id, record_version, record_state,
                    record_digest, metadata_json
             FROM project_release_history_reference
             WHERE release_id = ? ORDER BY ordinal ASC",
        )
        .bind(release_id)
        .fetch_all(self.db.pool())
        .await?;
        if rows.len() != expected.len() {
            return Err(invalid(
                "immutable release references do not match its snapshot",
            ));
        }
        for (row, (kind, id, version, state, digest, metadata)) in rows.into_iter().zip(expected) {
            if row.try_get::<String, _>("reference_kind")? != kind
                || row.try_get::<String, _>("record_id")? != id
                || row.try_get::<Option<String>, _>("record_version")? != version
                || row.try_get::<Option<String>, _>("record_state")? != state
                || row.try_get::<Option<String>, _>("record_digest")? != digest
                || row.try_get::<String, _>("metadata_json")? != metadata
            {
                return Err(invalid(
                    "immutable release reference disagrees with its snapshot",
                ));
            }
        }
        Ok(())
    }

    async fn release_pins(
        &self,
        project_id: &str,
        release_id: &str,
        frozen_evidence_json: &str,
    ) -> crate::Result<(Vec<api_types::EvidencePin>, Vec<api_types::EvidencePin>)> {
        let frozen: Vec<api_types::EvidencePin> =
            parse_json_required(frozen_evidence_json, "release evidence_references")?;
        let rows = sqlx::query(
            "SELECT p.id, p.release_id, p.attachment_id, p.asset_id,
                    p.asset_checksum, p.attachment_digest, p.availability,
                    p.pin_digest, p.created_at, m.checksum AS live_asset_checksum,
                    (SELECT t.availability FROM media_asset_tombstone t
                     WHERE t.asset_id = p.asset_id
                       AND (t.release_pin_id = p.id OR
                            (t.release_pin_id IS NULL AND t.release_id = p.release_id))
                     ORDER BY t.created_at DESC, t.id DESC LIMIT 1) AS tombstone_availability
             FROM project_release_history_media_pin p
             JOIN media_asset m ON m.id = p.asset_id
             WHERE p.project_id = ? AND p.release_id = ? ORDER BY p.id ASC",
        )
        .bind(project_id)
        .bind(release_id)
        .fetch_all(self.db.pool())
        .await?;
        let pins = rows
            .into_iter()
            .map(|row| {
                let attachment_id = row
                    .try_get::<Option<String>, _>("attachment_id")?
                    .ok_or_else(|| invalid("immutable release pin has no attachment identity"))?;
                let attachment_digest =
                    required_text(&row, "attachment_digest", "attachment digest")?;
                let asset_checksum = required_text(&row, "asset_checksum", "asset checksum")?;
                if let Some(checksum) = row
                    .try_get::<Option<String>, _>("live_asset_checksum")?
                    .filter(|value| !value.trim().is_empty())
                {
                    if checksum != asset_checksum {
                        return Err(crate::ServiceError::Db(db::DbError::VersionConflict));
                    }
                }
                let availability =
                    evidence_availability(&row.try_get::<String, _>("availability")?)?;
                let pin_digest = required_text(&row, "pin_digest", "pin digest")?;
                let availability_projection =
                    match row.try_get::<Option<String>, _>("tombstone_availability")? {
                        Some(value) => release_evidence_tombstone(&value)?,
                        None => release_evidence_availability(availability),
                    };
                let id: String = row.try_get("id")?;
                let frozen_pin = frozen
                    .iter()
                    .find(|pin| pin.id == id)
                    .ok_or_else(|| invalid(format!("release pin {id} has no frozen metadata")))?;
                if frozen_pin.attachment_digest != attachment_digest
                    || frozen_pin.asset_checksum != asset_checksum
                    || frozen_pin.attachment_id != attachment_id
                    || frozen_pin.asset_id != row.try_get::<String, _>("asset_id")?
                    || frozen_pin.release_id != row.try_get::<String, _>("release_id")?
                    || frozen_pin.availability != availability
                    || frozen_pin.availability_projection
                        != release_evidence_availability(availability)
                    || frozen_pin.pinned_at != row.try_get::<String, _>("created_at")?
                {
                    return Err(crate::ServiceError::Db(db::DbError::VersionConflict));
                }
                let expected_digest = canonical_digest_with_schema(
                    RELEASE_DIGEST_SCHEMA_VERSION,
                    &serde_json::to_string(frozen_pin).map_err(json_error)?,
                )
                .map_err(json_error)?;
                if pin_digest != expected_digest {
                    return Err(invalid(
                        "immutable release pin digest does not match its metadata",
                    ));
                }
                let projected_pin = api_types::EvidencePin {
                    id,
                    release_id: row.try_get("release_id")?,
                    attachment_id,
                    asset_id: row.try_get("asset_id")?,
                    attachment_digest,
                    asset_checksum,
                    availability,
                    availability_projection,
                    task_media_id: frozen_pin.task_media_id.clone(),
                    stable_project_url: frozen_pin.stable_project_url.clone(),
                    pinned_at: row.try_get("created_at")?,
                };
                Ok((frozen_pin.clone(), projected_pin))
            })
            .collect::<crate::Result<Vec<_>>>()?;
        if pins.len() != frozen.len() {
            return Err(invalid(
                "release evidence pin set does not match frozen metadata",
            ));
        }
        let (frozen, projected): (Vec<_>, Vec<_>) = pins.into_iter().unzip();
        Ok((frozen, projected))
    }
}

fn release_snapshot_digest(snapshot: &ReleaseSnapshot) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(snapshot)?;
    if let serde_json::Value::Object(object) = &mut value {
        object.remove("snapshot_digest");
    }
    canonical_digest_with_schema(RELEASE_DIGEST_SCHEMA_VERSION, &value)
}

fn release_record_from_row(row: SqliteRow) -> db::Result<ProjectReleaseRecord> {
    Ok(ProjectReleaseRecord {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        milestone_id: row.try_get("milestone_id")?,
        release_sequence: row.try_get("release_sequence")?,
        release_revision: row.try_get("release_revision")?,
        release_identifier: row.try_get("release_identifier")?,
        milestone_revision_id: row.try_get("milestone_revision_id")?,
        readiness_snapshot_id: row.try_get("readiness_snapshot_id")?,
        readiness_digest: row.try_get("readiness_digest")?,
        baseline_id: row.try_get("baseline_id")?,
        baseline_revision_id: row.try_get("baseline_revision_id")?,
        baseline_digest: row.try_get("baseline_digest")?,
        release_policy_revision: row.try_get("release_policy_revision")?,
        release_policy_digest: row.try_get("release_policy_digest")?,
        summary: row.try_get("summary")?,
        changelog: row.try_get("changelog")?,
        known_issues_json: row.try_get("known_issues_json")?,
        charter_revision_id: row.try_get("charter_revision_id")?,
        document_revisions_json: row.try_get("document_revisions_json")?,
        decision_ids_json: row.try_get("decision_ids_json")?,
        task_references_json: row.try_get("task_references_json")?,
        validation_references_json: row.try_get("validation_references_json")?,
        git_references_json: row.try_get("git_references_json")?,
        evidence_references_json: row.try_get("evidence_references_json")?,
        waivers_json: row.try_get("waivers_json")?,
        releasing_principal_type: row.try_get("releasing_principal_type")?,
        releasing_principal_id: row.try_get("releasing_principal_id")?,
        authorization_basis: row.try_get("authorization_basis")?,
        authorization_action: row.try_get("authorization_action")?,
        authorization_occurred_at: row.try_get("authorization_occurred_at")?,
        explicit_event: row.try_get("explicit_event")?,
        schema_version: row.try_get("schema_version")?,
        snapshot_digest: row.try_get("snapshot_digest")?,
        idempotency_key: row.try_get("idempotency_key")?,
        created_at: row.try_get("created_at")?,
    })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct RepositoryContextReference {
    task_id: String,
    task_version: i64,
    repository_id: String,
    repository_name: String,
    repository_kind: String,
    remote_url: Option<String>,
    default_branch: String,
    execution_id: Option<String>,
    execution_status: Option<String>,
    execution_role: Option<String>,
    before_sha: Option<String>,
    after_sha: Option<String>,
    execution_summary: Option<String>,
    execution_created_at: Option<String>,
    execution_updated_at: Option<String>,
    review_id: Option<String>,
    review_status: Option<String>,
    ci_results: Option<String>,
    audit_result: Option<String>,
    human_decision: Option<String>,
    review_created_at: Option<String>,
    review_updated_at: Option<String>,
    observed_at: String,
}

fn validate_repository_context_reference(
    reference: &RepositoryContextReference,
) -> crate::Result<()> {
    if reference.task_id.trim().is_empty()
        || reference.task_version <= 0
        || reference.repository_id.trim().is_empty()
        || reference.repository_name.trim().is_empty()
        || reference.repository_kind.trim().is_empty()
        || reference.default_branch.trim().is_empty()
        || reference.observed_at.trim().is_empty()
        || reference
            .remote_url
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
    {
        return Err(invalid(
            "repository context has incomplete historical identity",
        ));
    }
    if reference.execution_id.is_none()
        && (reference.execution_status.is_some()
            || reference.execution_role.is_some()
            || reference.before_sha.is_some()
            || reference.after_sha.is_some()
            || reference.execution_summary.is_some()
            || reference.execution_created_at.is_some()
            || reference.execution_updated_at.is_some())
    {
        return Err(invalid(
            "repository context has execution fields without an identity",
        ));
    }
    if reference.review_id.is_none()
        && (reference.review_status.is_some()
            || reference.ci_results.is_some()
            || reference.audit_result.is_some()
            || reference.human_decision.is_some()
            || reference.review_created_at.is_some()
            || reference.review_updated_at.is_some())
    {
        return Err(invalid(
            "repository context has review fields without an identity",
        ));
    }
    Ok(())
}

fn validate_persisted_authorization(
    authorization: &AuthorizationProvenance,
    expected_action: &str,
) -> crate::Result<()> {
    if authorization.principal.id.trim().is_empty()
        || authorization.authorization_basis.trim().is_empty()
        || authorization.action != expected_action
        || authorization.event_id.trim().is_empty()
        || !well_formed_authorization_timestamp(&authorization.occurred_at)
    {
        return Err(invalid(
            "corrupt immutable release authorization provenance",
        ));
    }
    Ok(())
}

fn well_formed_authorization_timestamp(value: &str) -> bool {
    value.len() <= MAX_AUTHORIZATION_TIMESTAMP_LEN
        && value.trim() == value
        && DateTime::parse_from_rfc3339(value).is_ok()
}

fn principal_kind(value: &str) -> crate::Result<PrincipalKind> {
    match value {
        "user" => Ok(PrincipalKind::User),
        "agent" => Ok(PrincipalKind::Agent),
        "worker" => Ok(PrincipalKind::Worker),
        "reviewer" => Ok(PrincipalKind::Reviewer),
        "service" => Ok(PrincipalKind::Service),
        "system" => Ok(PrincipalKind::System),
        other => Err(invalid(format!("unknown persisted principal kind {other}"))),
    }
}

fn evidence_availability(value: &str) -> crate::Result<api_types::EvidenceAvailability> {
    match value {
        "available" => Ok(api_types::EvidenceAvailability::Available),
        "quarantined" => Ok(api_types::EvidenceAvailability::Quarantined),
        "redacted" => Ok(api_types::EvidenceAvailability::Redacted),
        "purged" => Ok(api_types::EvidenceAvailability::Purged),
        other => Err(invalid(format!(
            "unknown persisted evidence availability {other}"
        ))),
    }
}

fn release_evidence_availability(
    value: api_types::EvidenceAvailability,
) -> api_types::ReleaseEvidenceAvailability {
    match value {
        api_types::EvidenceAvailability::Available => {
            api_types::ReleaseEvidenceAvailability::Available
        }
        api_types::EvidenceAvailability::Quarantined => {
            api_types::ReleaseEvidenceAvailability::Quarantined
        }
        api_types::EvidenceAvailability::Redacted => {
            api_types::ReleaseEvidenceAvailability::Redacted
        }
        api_types::EvidenceAvailability::Purged => api_types::ReleaseEvidenceAvailability::Purged,
    }
}

fn release_evidence_tombstone(
    value: &str,
) -> crate::Result<api_types::ReleaseEvidenceAvailability> {
    match value {
        "redacted" => Ok(api_types::ReleaseEvidenceAvailability::Redacted),
        "purged" | "evidence_unavailable" => {
            Ok(api_types::ReleaseEvidenceAvailability::EvidenceUnavailable)
        }
        other => Err(invalid(format!(
            "unknown persisted media tombstone availability {other}"
        ))),
    }
}

fn required_text(row: &SqliteRow, column: &str, label: &str) -> crate::Result<String> {
    row.try_get::<Option<String>, _>(column)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(format!("immutable release pin has no {label}")))
}

fn parse_json_required<T: DeserializeOwned>(value: &str, field: &str) -> crate::Result<T> {
    serde_json::from_str(value)
        .map_err(|error| invalid(format!("corrupt immutable {field} JSON: {error}")))
}

fn json_error(error: impl std::fmt::Display) -> crate::ServiceError {
    invalid(format!("invalid canonical historical JSON: {error}"))
}

fn invalid(message: impl Into<String>) -> crate::ServiceError {
    crate::ServiceError::InvalidOperation {
        message: message.into(),
    }
}
