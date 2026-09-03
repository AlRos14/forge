const RESULT_INSTRUCTION: &str = r#"End with exactly one machine-readable line:
FORGE_RESULT: {"schema_version":1,"kind":"review","verdict":"pass|fail|needs_human","summary":"...","findings":[{"severity":"blocking|non_blocking","evidence":"...","expected":"...","actual":"..."}],"questions":[]}
Use needs_human when correctness depends on missing product, policy, scope, or risk authority. Missing or invalid structured output is a protocol failure."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditorVerdict {
    Passed,
    Failed { reason: String },
    NeedsHuman { reason: String },
}

pub fn render_auditor_prompt(
    task_title: &str,
    task_description: Option<&str>,
    diff_text: &str,
    override_template: Option<&str>,
) -> String {
    match override_template {
        Some(template) => append_marker_instruction(template),
        None => {
            let description = task_description
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("(no description)");
            format!(
                "Review this task implementation.\n\nTask title:\n{task_title}\n\nTask description:\n{description}\n\nGit diff:\n```diff\n{diff_text}\n```\n\n{RESULT_INSTRUCTION}"
            )
        }
    }
}

pub fn parse_verdict(final_message: &str) -> AuditorVerdict {
    let Some(payload) = final_message
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("FORGE_RESULT: "))
    else {
        return AuditorVerdict::Failed {
            reason: "structured review result missing".to_owned(),
        };
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return AuditorVerdict::Failed {
            reason: "structured review result is invalid JSON".to_owned(),
        };
    };
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_i64)
        != Some(1)
        || value.get("kind").and_then(serde_json::Value::as_str) != Some("review")
    {
        return AuditorVerdict::Failed {
            reason: "structured review result has an unsupported contract".to_owned(),
        };
    }
    let summary = value
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("review supplied no summary")
        .to_owned();
    match value.get("verdict").and_then(serde_json::Value::as_str) {
        Some("pass") => AuditorVerdict::Passed,
        Some("fail") => AuditorVerdict::Failed { reason: summary },
        Some("needs_human") => AuditorVerdict::NeedsHuman { reason: summary },
        _ => AuditorVerdict::Failed {
            reason: "structured review verdict is invalid".to_owned(),
        },
    }
}

fn append_marker_instruction(template: &str) -> String {
    let separator = if template.ends_with('\n') { "" } else { "\n\n" };
    format!("{template}{separator}{RESULT_INSTRUCTION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_pass_parses_as_passed() {
        assert_eq!(
            parse_verdict("Looks good.\nFORGE_RESULT: {\"schema_version\":1,\"kind\":\"review\",\"verdict\":\"pass\",\"summary\":\"all good\",\"findings\":[],\"questions\":[]}"),
            AuditorVerdict::Passed
        );
    }

    #[test]
    fn structured_fail_captures_reason() {
        assert_eq!(
            parse_verdict("FORGE_RESULT: {\"schema_version\":1,\"kind\":\"review\",\"verdict\":\"fail\",\"summary\":\"missing null check\",\"findings\":[],\"questions\":[]}"),
            AuditorVerdict::Failed {
                reason: "missing null check".to_owned()
            }
        );
    }

    #[test]
    fn structured_human_escalation_is_distinct() {
        assert_eq!(
            parse_verdict("FORGE_RESULT: {\"schema_version\":1,\"kind\":\"review\",\"verdict\":\"needs_human\",\"summary\":\"policy choice required\",\"findings\":[],\"questions\":[]}"),
            AuditorVerdict::NeedsHuman {
                reason: "policy choice required".to_owned()
            }
        );
    }

    #[test]
    fn missing_marker_fails() {
        assert_eq!(
            parse_verdict("Looks fine."),
            AuditorVerdict::Failed {
                reason: "structured review result missing".to_owned()
            }
        );
    }

    #[test]
    fn override_template_appends_structured_result_instruction() {
        let prompt = render_auditor_prompt("ignored", None, "diff", Some("Use my rubric."));

        assert!(prompt.starts_with("Use my rubric."));
        assert!(prompt.contains("FORGE_RESULT:"));
        assert!(prompt.contains("pass|fail|needs_human"));
    }
}
