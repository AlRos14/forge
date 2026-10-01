//! Markdown parsing and read-only presentation for generic Plan Artifacts.
//!
//! This module deliberately has no filesystem or legacy plan-table access. The
//! Artifact row and its Execution producer are the only durable plan record.

use api_types::{PlanArtifactDetail, PlanChecklistItem, PlanProgressSummary};
use db::{
    Artifact, ArtifactKind, ArtifactStorageKind, CollaborationRepo, ExecutionPurpose,
    ExecutionRepo, PageRequest, SortBy, SortOrder, SqliteDb,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPlanItem {
    pub checked: bool,
    pub label: String,
    pub nesting_level: usize,
    pub line_number: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPlanArtifact {
    pub markdown: String,
    pub items: Vec<ParsedPlanItem>,
    pub warnings: Vec<String>,
}

/// Parse checkboxes for display only. No lifecycle or dispatch code may use
/// the derived progress as an execution gate.
pub fn parse_plan_markdown(content: &str) -> ParsedPlanArtifact {
    let mut items = Vec::new();
    let mut warnings = Vec::new();

    for (line_index, line) in content.lines().enumerate() {
        let line_number = line_index + 1;
        if let Some(item) = parse_checkbox_line(line, line_number) {
            items.push(item);
        } else if looks_like_checkbox_line(line) {
            warnings.push(format!("line {line_number}: malformed checkbox item"));
        }
    }

    ParsedPlanArtifact {
        markdown: content.to_owned(),
        items,
        warnings,
    }
}

/// Read a Task's generic plan Artifacts for the compatibility projection.
/// Consumers that execute work must use an explicitly selected Artifact id.
pub async fn list_plan_artifacts(
    db: &SqliteDb,
    task_id: &str,
) -> crate::Result<Vec<PlanArtifactDetail>> {
    let page = CollaborationRepo::list_artifacts_by_kind(
        db,
        task_id,
        ArtifactKind::Plan,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    page.items.iter().map(to_plan_artifact_detail).collect()
}

pub async fn latest_plan_artifact(
    db: &SqliteDb,
    task_id: &str,
) -> crate::Result<Option<PlanArtifactDetail>> {
    Ok(list_plan_artifacts(db, task_id).await?.into_iter().next())
}

/// Resolve Plan Artifact context from one exact Execution identity. A Plan
/// producer contributes its output; any other Execution contributes only its
/// already-pinned Plan inputs.
pub async fn plan_artifacts_for_execution(
    db: &SqliteDb,
    task_id: &str,
    execution_id: &str,
) -> crate::Result<Vec<Artifact>> {
    let Some(execution) = ExecutionRepo::get_by_id(db, execution_id).await? else {
        return Ok(Vec::new());
    };
    if execution.task_id != task_id {
        return Err(crate::ServiceError::invalid_operation(
            "Execution belongs to another Task",
        ));
    }
    if execution.purpose == Some(ExecutionPurpose::Plan) {
        return Ok(CollaborationRepo::get_execution_artifact_output(
            db,
            execution_id,
            ArtifactKind::Plan,
        )
        .await?
        .into_iter()
        .collect());
    }

    let mut artifacts = Vec::new();
    for input in CollaborationRepo::list_execution_artifact_inputs(db, execution_id).await? {
        if let Some(artifact) = CollaborationRepo::get_artifact(db, &input.artifact_id).await? {
            if artifact.task_id == task_id && artifact.kind == ArtifactKind::Plan {
                artifacts.push(artifact);
            }
        }
    }
    Ok(artifacts)
}

pub fn to_plan_artifact_detail(artifact: &Artifact) -> crate::Result<PlanArtifactDetail> {
    let markdown = match (artifact.storage_kind, artifact.content.as_deref()) {
        (ArtifactStorageKind::Inline, Some(content)) => content.to_owned(),
        _ => String::new(),
    };
    let parsed = parse_plan_markdown(&markdown);
    let mut warnings = parsed.warnings;
    if markdown.is_empty() {
        warnings.push("Plan Artifact content is unavailable in this projection".to_owned());
    }
    let producer = match &artifact.producer {
        db::ActorRef::Human(id) => api_types::ActorRef::Human(id.clone()),
        db::ActorRef::Agent(id) => api_types::ActorRef::Agent(id.clone()),
    };

    Ok(PlanArtifactDetail {
        artifact_id: artifact.id.clone(),
        task_id: artifact.task_id.clone(),
        producer_execution_id: artifact.producer_execution_id.clone(),
        producer,
        content_digest: artifact.digest.clone(),
        markdown,
        items: parsed
            .items
            .into_iter()
            .map(|item| PlanChecklistItem {
                checked: item.checked,
                label: item.label,
                nesting_level: u32::try_from(item.nesting_level).unwrap_or(u32::MAX),
                line_number: u32::try_from(item.line_number).unwrap_or(u32::MAX),
            })
            .collect(),
        warnings,
        created_at: artifact.created_at.clone(),
    })
}

pub fn to_plan_progress_summary(artifact: &PlanArtifactDetail) -> PlanProgressSummary {
    let total = u32::try_from(artifact.items.len()).unwrap_or(u32::MAX);
    let completed = u32::try_from(artifact.items.iter().filter(|item| item.checked).count())
        .unwrap_or(u32::MAX);
    PlanProgressSummary {
        total,
        completed,
        remaining: total.saturating_sub(completed),
        available: true,
        warnings: artifact.warnings.clone(),
    }
}

fn parse_checkbox_line(line: &str, line_number: usize) -> Option<ParsedPlanItem> {
    let leading_spaces = line.bytes().take_while(|byte| *byte == b' ').count();
    let rest = &line[leading_spaces..];
    let bytes = rest.as_bytes();
    if bytes.len() < 6 || !matches!(bytes[0], b'-' | b'*') || bytes[1] != b' ' || bytes[2] != b'[' {
        return None;
    }
    let checked = match bytes[3] {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    if bytes[4] != b']' || bytes[5] != b' ' {
        return None;
    }
    Some(ParsedPlanItem {
        checked,
        label: rest[6..].trim().to_owned(),
        nesting_level: leading_spaces / 2,
        line_number,
    })
}

fn looks_like_checkbox_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- [") || trimmed.starts_with("* [")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_checklist_as_display_projection() {
        let parsed = parse_plan_markdown(
            "# Plan\n- [ ] root\n  - [x] child\n    * [X] grandchild\n - [o] malformed",
        );
        assert_eq!(parsed.items.len(), 3);
        assert_eq!(parsed.items[0].line_number, 2);
        assert_eq!(parsed.items[1].nesting_level, 1);
        assert_eq!(parsed.items[2].nesting_level, 2);
        assert_eq!(parsed.warnings, vec!["line 5: malformed checkbox item"]);
    }

    #[test]
    fn progress_is_derived_from_projection_only() {
        let mut detail = PlanArtifactDetail {
            artifact_id: "artifact".to_owned(),
            task_id: "task".to_owned(),
            producer_execution_id: "execution".to_owned(),
            producer: api_types::ActorRef::Human("user".to_owned()),
            content_digest: Some("digest".to_owned()),
            markdown: "- [x] done\n- [ ] pending".to_owned(),
            items: vec![
                PlanChecklistItem {
                    checked: true,
                    label: "done".to_owned(),
                    nesting_level: 0,
                    line_number: 1,
                },
                PlanChecklistItem {
                    checked: false,
                    label: "pending".to_owned(),
                    nesting_level: 0,
                    line_number: 2,
                },
            ],
            warnings: vec![],
            created_at: "2026-10-01T00:00:00Z".to_owned(),
        };
        let progress = to_plan_progress_summary(&detail);
        assert_eq!(
            (progress.total, progress.completed, progress.remaining),
            (2, 1, 1)
        );
        detail.items.clear();
        assert_eq!(to_plan_progress_summary(&detail).remaining, 0);
    }
}
