use uuid::{Uuid, Version};

#[must_use]
pub fn new_uuid_v4() -> String {
    Uuid::new_v4().to_string()
}

#[must_use]
pub fn validate_uuid_v4(value: &str) -> bool {
    Uuid::parse_str(value)
        .map(|uuid| uuid.get_version() == Some(Version::Random))
        .unwrap_or(false)
}
