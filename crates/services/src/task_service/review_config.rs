use super::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ReviewConfigParts {
    pub(super) ci_steps: Vec<String>,
    pub(super) review_prompt: Option<String>,
    pub(super) auditor_agent_id: Option<String>,
}

pub(super) fn review_config_from_json(review_config: Option<&str>) -> Result<ReviewConfigParts> {
    let Some(review_config) = review_config else {
        return Ok(ReviewConfigParts::default());
    };
    let value: Value = serde_json::from_str(review_config).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid review_config: {error}"))
    })?;
    let value = value.get("review").cloned().unwrap_or(value);
    let ci_steps = match value.get("ci_steps") {
        Some(steps) => {
            let Some(steps) = steps.as_array() else {
                return Err(ServiceError::invalid_operation(
                    "review_config.ci_steps must be an array",
                ));
            };
            steps
                .iter()
                .map(|step| {
                    step.as_str().map(str::to_owned).ok_or_else(|| {
                        ServiceError::invalid_operation(
                            "review_config.ci_steps entries must be strings",
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?
        }
        None => Vec::new(),
    };
    let review_prompt = optional_string_field(&value, "review_prompt")?;
    let auditor_agent_id = optional_string_field(&value, "auditor_agent_id")?;
    Ok(ReviewConfigParts {
        ci_steps,
        review_prompt,
        auditor_agent_id,
    })
}

fn optional_string_field(value: &Value, field: &'static str) -> Result<Option<String>> {
    match value.get(field) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(ServiceError::invalid_operation(format!(
            "review_config.{field} must be a string"
        ))),
    }
}
