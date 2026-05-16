const MARKER_INSTRUCTION: &str = "End your response with exactly one of:\n===REVIEW: PASS===\n===REVIEW: FAIL: <short reason>===";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditorVerdict {
    Passed,
    Failed { reason: String },
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
                "Review this task implementation.\n\nTask title:\n{task_title}\n\nTask description:\n{description}\n\nGit diff:\n```diff\n{diff_text}\n```\n\n{MARKER_INSTRUCTION}"
            )
        }
    }
}

pub fn parse_verdict(final_message: &str) -> AuditorVerdict {
    if final_message.contains("===REVIEW: PASS===") {
        return AuditorVerdict::Passed;
    }

    if let Some(start) = final_message.find("===REVIEW: FAIL: ") {
        let reason_start = start + "===REVIEW: FAIL: ".len();
        if let Some(end) = final_message[reason_start..].find("===") {
            let reason = &final_message[reason_start..reason_start + end];
            if !reason.is_empty() {
                return AuditorVerdict::Failed {
                    reason: reason.to_owned(),
                };
            }
        }
    }

    AuditorVerdict::Failed {
        reason: "verdict marker missing".to_owned(),
    }
}

fn append_marker_instruction(template: &str) -> String {
    let separator = if template.ends_with('\n') { "" } else { "\n\n" };
    format!("{template}{separator}{MARKER_INSTRUCTION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_marker_parses_as_passed() {
        assert_eq!(
            parse_verdict("Looks good.\n===REVIEW: PASS==="),
            AuditorVerdict::Passed
        );
    }

    #[test]
    fn fail_marker_captures_reason() {
        assert_eq!(
            parse_verdict("No.\n===REVIEW: FAIL: missing null check==="),
            AuditorVerdict::Failed {
                reason: "missing null check".to_owned()
            }
        );
    }

    #[test]
    fn missing_marker_fails() {
        assert_eq!(
            parse_verdict("Looks fine."),
            AuditorVerdict::Failed {
                reason: "verdict marker missing".to_owned()
            }
        );
    }

    #[test]
    fn override_template_appends_marker_instruction() {
        let prompt = render_auditor_prompt("ignored", None, "diff", Some("Use my rubric."));

        assert!(prompt.starts_with("Use my rubric."));
        assert!(prompt.contains("===REVIEW: PASS==="));
        assert!(prompt.contains("===REVIEW: FAIL: <short reason>==="));
    }
}
