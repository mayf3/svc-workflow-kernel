pub(crate) mod health;
pub(crate) mod instances;
pub(crate) mod timeline;
pub(crate) mod transitions;

use axum::http::HeaderMap;
use uuid::Uuid;

use crate::auth::AuthenticatedPrincipal;
use crate::http::error::ApiError;

fn require_scope(
    principal: &AuthenticatedPrincipal,
    required: &'static str,
) -> Result<(), ApiError> {
    if principal.has_scope(required) {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

fn idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers.get("idempotency-key").ok_or_else(|| {
        ApiError::bad_request(
            "missing_idempotency_key",
            "Idempotency-Key header is required",
        )
    })?;
    let value = value.to_str().map_err(|_| {
        ApiError::bad_request(
            "invalid_idempotency_key",
            "Idempotency-Key header is invalid",
        )
    })?;
    if value.is_empty()
        || value.len() > 128
        || !value
            .as_bytes()
            .iter()
            .all(|byte| (0x21..=0x7e).contains(byte))
    {
        return Err(ApiError::bad_request(
            "invalid_idempotency_key",
            "Idempotency-Key must be 1-128 visible ASCII characters",
        ));
    }
    Ok(value.to_string())
}

fn path_uuid(value: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value).map_err(|_| {
        ApiError::bad_request(
            "invalid_path_parameter",
            "workflowInstanceId must be a UUID",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn validates_idempotency_key_contract() {
        let mut headers = HeaderMap::new();
        assert_eq!(
            idempotency_key(&headers).unwrap_err().code(),
            "missing_idempotency_key"
        );
        headers.insert("idempotency-key", HeaderValue::from_static(" "));
        assert_eq!(
            idempotency_key(&headers).unwrap_err().code(),
            "invalid_idempotency_key"
        );
        headers.insert("idempotency-key", HeaderValue::from_static("command-123"));
        assert_eq!(idempotency_key(&headers).unwrap(), "command-123");
    }
}
