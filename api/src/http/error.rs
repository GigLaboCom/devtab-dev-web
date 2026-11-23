//! The API's error type and its JSON shape:
//! `{"error":"<kebab-case code>","message":"<English text>"}`.

use axum::Json;
use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("not found")]
    NotFound,
    #[error("request body is too large")]
    PayloadTooLarge,
    /// Answered with `Retry-After` in whole seconds.
    #[error("too many requests")]
    RateLimited { retry_after_s: u64 },
    #[error("service unavailable")]
    Unavailable,
    /// Details are logged, never sent to the client.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad-request",
            Self::NotFound => "not-found",
            Self::PayloadTooLarge => "payload-too-large",
            Self::RateLimited { .. } => "rate-limited",
            Self::Unavailable => "unavailable",
            Self::Internal(_) => "internal",
        }
    }
}

/// The error body as JSON.
pub fn error_json(code: &str, message: &str) -> Value {
    json!({ "error": code, "message": message })
}

/// Builds the response of every error the API answers.
pub fn error_body(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(error_json(code, message))).into_response()
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let message = match &self {
            Self::Internal(err) => {
                tracing::error!(error = ?err, "internal error");
                "internal error".to_owned()
            }
            other => other.to_string(),
        };
        let mut response = error_body(self.status(), self.code(), &message);
        if let Self::RateLimited { retry_after_s } = self {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(retry_after_s));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_carries_retry_after_in_seconds() {
        let response = AppError::RateLimited { retry_after_s: 42 }.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[RETRY_AFTER], "42");
    }

    #[test]
    fn unavailable_has_no_retry_after() {
        let response = AppError::Unavailable.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().get(RETRY_AFTER).is_none());
    }
}
