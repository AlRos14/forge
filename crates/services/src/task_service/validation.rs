use super::*;

pub(super) fn validate_required(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(ServiceError::invalid_operation(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

pub(super) fn serialize_config(config: Option<Value>) -> Result<Option<String>> {
    config
        .map(|value| serde_json::to_string(&value))
        .transpose()
        .map_err(|error| ServiceError::invalid_operation(format!("invalid JSON config: {error}")))
}
