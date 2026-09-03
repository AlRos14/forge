use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use serde::de::DeserializeOwned;

use crate::errors::ApiError;

/// JSON body that treats a missing or empty payload as `T::default()`.
///
/// Axum's `Option<Json<T>>` still fails when the client sends
/// `Content-Type: application/json` with an empty body (EOF). The webapp
/// does that on several body-less POSTs, including task Cancel.
pub struct OptionalJson<T>(pub T);

pub fn parse_optional_json<T: Default + DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    let trimmed = bytes.trim_ascii();
    if trimmed.is_empty() || trimmed == b"null" {
        return Ok(T::default());
    }

    serde_json::from_slice(trimmed).map_err(|error| {
        ApiError::bad_request(format!("Failed to parse the request body as JSON: {error}"))
    })
}

impl<S, T> FromRequest<S> for OptionalJson<T>
where
    T: Default + DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        parse_optional_json(&bytes).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_types::TaskActionRequest;

    #[test]
    fn empty_body_is_default() {
        let parsed = parse_optional_json::<TaskActionRequest>(b"").expect("empty body");
        assert_eq!(parsed.reason, None);
        assert_eq!(parsed.version, None);
    }

    #[test]
    fn whitespace_body_is_default() {
        let parsed = parse_optional_json::<TaskActionRequest>(b"  \n").expect("whitespace");
        assert_eq!(parsed.reason, None);
    }

    #[test]
    fn null_body_is_default() {
        let parsed = parse_optional_json::<TaskActionRequest>(b"null").expect("null");
        assert_eq!(parsed.version, None);
    }

    #[test]
    fn null_body_with_surrounding_whitespace_is_default() {
        let parsed = parse_optional_json::<TaskActionRequest>(b" \tnull\r\n").expect("padded null");
        assert_eq!(parsed.reason, None);
        assert_eq!(parsed.version, None);
    }

    #[test]
    fn object_body_is_parsed() {
        let parsed = parse_optional_json::<TaskActionRequest>(br#"{"reason":"stop","version":3}"#)
            .expect("object");
        assert_eq!(parsed.reason.as_deref(), Some("stop"));
        assert_eq!(parsed.version, Some(3));
    }

    #[test]
    fn invalid_json_is_rejected() {
        let error = parse_optional_json::<TaskActionRequest>(b"{").expect_err("invalid");
        assert!(format!("{error:?}").contains("Failed to parse the request body as JSON"));
    }
}
