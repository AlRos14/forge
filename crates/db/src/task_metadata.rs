use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Task;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordered_sequence_started: Option<bool>,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

impl TaskMetadata {
    pub fn parse(raw: Option<&str>) -> Result<Self, serde_json::Error> {
        raw.map(serde_json::from_str)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub fn to_json(&self) -> Option<String> {
        if self.ordered_sequence_started.is_none() && self.extra.is_empty() {
            return None;
        }
        Some(serde_json::to_string(self).expect("task metadata serialization is infallible"))
    }
}

impl Task {
    pub fn metadata(&self) -> Result<TaskMetadata, serde_json::Error> {
        TaskMetadata::parse(self.metadata_json.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_metadata_parse_preserves_known_and_extra_fields() {
        let metadata = TaskMetadata::parse(Some(r#"{"ordered_sequence_started":true,"custom":7}"#))
            .expect("metadata parses");

        assert_eq!(metadata.ordered_sequence_started, Some(true));
        assert_eq!(metadata.extra.get("custom"), Some(&Value::from(7)));
        assert_eq!(
            metadata.to_json().as_deref(),
            Some(r#"{"ordered_sequence_started":true,"custom":7}"#)
        );
    }

    #[test]
    fn task_metadata_to_json_omits_empty_metadata() {
        assert_eq!(TaskMetadata::default().to_json(), None);
    }
}
