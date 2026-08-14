//! Pure Project Document rendering, digest, and revision-diff helpers.
//!
//! Project Documents are Forge-owned artifacts.  This module deliberately
//! contains no authorization or persistence: callers must establish Project
//! scope and approval policy before invoking the helpers, while this module
//! guarantees that the canonical JSON/Markdown views are deterministic.

use api_types::{
    canonical_json, canonical_render_digest, ProjectDocumentContent, ProjectDocumentKind,
};

/// Renderer revision included in every Project Document render digest.
pub const PROJECT_DOCUMENT_RENDER_VERSION: &str = "forge.project-document/v1";

/// Schema version included in every Project Document content digest.
pub const PROJECT_DOCUMENT_SCHEMA_VERSION: &str = "forge.project-document-content/v1";

/// Compute the digest of the typed, canonical Project Document content.
pub fn document_content_digest(content: &ProjectDocumentContent) -> String {
    canonical_digest_with_document_schema(content)
}

/// Compute the digest of the exact rendered view and renderer revision.
pub fn document_render_digest(render_version: &str, rendered_view: &str) -> String {
    canonical_render_digest(render_version, rendered_view)
        .expect("Project Document render view is serializable")
}

/// Render a typed Project Document into a deterministic, safe Markdown view.
///
/// The canonical JSON is placed in an HTML `<pre>` block rather than a normal
/// Markdown code fence.  This keeps model/user content containing backticks,
/// headings, or HTML from becoming executable Markdown structure while still
/// providing a copyable file-like view.
pub fn render_project_document(
    title: &str,
    kind: ProjectDocumentKind,
    content: &ProjectDocumentContent,
) -> String {
    let canonical = canonical_document_json(content);
    let mut rendered = String::new();
    rendered.push_str("# ");
    rendered.push_str(&escape_markdown_text(title));
    rendered.push_str("\n\n");
    rendered.push_str("- Document kind: `");
    rendered.push_str(document_kind_name(kind));
    rendered.push_str("`\n");
    rendered.push_str("- Schema version: `");
    rendered.push_str(PROJECT_DOCUMENT_SCHEMA_VERSION);
    rendered.push_str("`\n");
    rendered.push_str("- Render version: `");
    rendered.push_str(PROJECT_DOCUMENT_RENDER_VERSION);
    rendered.push_str("`\n\n");
    rendered.push_str("## Canonical content\n\n<pre>");
    rendered.push_str(&escape_html(&canonical));
    rendered.push_str("</pre>\n");
    rendered
}

/// Return canonical JSON suitable for the JSON export view.
pub fn render_project_document_json(content: &ProjectDocumentContent) -> String {
    canonical_document_json(content)
}

/// Produce a deterministic line-oriented diff between two rendered views.
///
/// This intentionally keeps the representation small and transport-safe.  A
/// revision diff is explanatory metadata; the immutable revision bodies and
/// their digests remain authoritative.
pub fn diff_project_document_views(base: Option<&str>, candidate: &str) -> String {
    let Some(base) = base else {
        return candidate
            .lines()
            .map(|line| format!("+{line}"))
            .collect::<Vec<_>>()
            .join("\n");
    };

    let before = base.lines().collect::<Vec<_>>();
    let after = candidate.lines().collect::<Vec<_>>();
    let mut output = Vec::new();
    let common = before.len().min(after.len());
    for index in 0..common {
        if before[index] == after[index] {
            output.push(format!(" {line}", line = before[index]));
        } else {
            output.push(format!("-{}", before[index]));
            output.push(format!("+{}", after[index]));
        }
    }
    for line in before.iter().skip(common) {
        output.push(format!("-{line}"));
    }
    for line in after.iter().skip(common) {
        output.push(format!("+{line}"));
    }
    output.join("\n")
}

pub fn document_kind_name(kind: ProjectDocumentKind) -> &'static str {
    match kind {
        ProjectDocumentKind::Research => "research",
        ProjectDocumentKind::DeliveryBrief => "delivery_brief",
        ProjectDocumentKind::ProductSpec => "product_spec",
        ProjectDocumentKind::Design => "design",
        ProjectDocumentKind::Architecture => "architecture",
        ProjectDocumentKind::ExecutionPlan => "execution_plan",
    }
}

pub fn parse_document_kind(value: &str) -> Option<ProjectDocumentKind> {
    match value {
        "research" => Some(ProjectDocumentKind::Research),
        "delivery_brief" => Some(ProjectDocumentKind::DeliveryBrief),
        "product_spec" => Some(ProjectDocumentKind::ProductSpec),
        "design" => Some(ProjectDocumentKind::Design),
        "architecture" => Some(ProjectDocumentKind::Architecture),
        "execution_plan" => Some(ProjectDocumentKind::ExecutionPlan),
        _ => None,
    }
}

pub fn parse_document_revision_lifecycle(value: &str) -> bool {
    matches!(value, "draft" | "proposed")
}

fn canonical_document_json(content: &ProjectDocumentContent) -> String {
    canonical_json(content).expect("ProjectDocumentContent is serializable")
}

fn canonical_digest_with_document_schema(content: &ProjectDocumentContent) -> String {
    api_types::canonical_digest_with_schema(PROJECT_DOCUMENT_SCHEMA_VERSION, content)
        .expect("ProjectDocumentContent is serializable")
}

fn escape_markdown_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('`', "\\`")
        .replace('#', "\\#")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(['\n', '\r'], " ")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_types::{
        DeliveryBriefContent, DocumentAcceptanceItem, DocumentPlanItem, ProjectDocumentContent,
    };

    fn content() -> ProjectDocumentContent {
        ProjectDocumentContent::DeliveryBrief(DeliveryBriefContent {
            intended_deliverables: vec!["A small release".to_owned()],
            boundaries: vec!["No hidden workspace access".to_owned()],
            plan_items: vec![DocumentPlanItem {
                id: "plan-1".to_owned(),
                outcome: "Ship".to_owned(),
                dependencies: vec![],
                task_ids: vec![],
            }],
            acceptance_matrix: vec![DocumentAcceptanceItem {
                id: "accept-1".to_owned(),
                statement: "It works".to_owned(),
                evidence: vec![],
                required: true,
            }],
            risks: vec![],
            rollback_and_recovery: vec!["Revert".to_owned()],
            adaptive_envelope: vec!["No destructive actions".to_owned()],
            governing_charter_revision_id: Some("charter-r1".to_owned()),
        })
    }

    #[test]
    fn content_digest_is_stable_for_object_key_order() {
        let first = content();
        let second = serde_json::from_str::<ProjectDocumentContent>(
            &serde_json::to_string(&first).expect("serialize"),
        )
        .expect("deserialize");
        assert_eq!(
            document_content_digest(&first),
            document_content_digest(&second)
        );
    }

    #[test]
    fn markdown_escapes_user_control_characters() {
        let rendered = render_project_document(
            "# inject `heading` <tag>",
            ProjectDocumentKind::DeliveryBrief,
            &content(),
        );
        assert!(rendered.contains(r"\# inject \`heading\` &lt;tag&gt;"));
        assert!(!rendered.contains("<tag>"));
    }

    #[test]
    fn diff_is_deterministic_and_shows_changed_lines() {
        let diff = diff_project_document_views(Some("same\nold"), "same\nnew");
        assert_eq!(diff, " same\n-old\n+new");
    }
}
