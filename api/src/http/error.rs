//! The API's error type and its JSON shape:
//! `{"error":"<kebab-case code>","message":"<English text>"}`. The codes of
//! the extension's endpoints are the contract's (`contract/licence-v1/errors.json`).

use axum::Json;
use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Value, json};
use utoipa::ToSchema;

/// The body of every error answer.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    /// A kebab-case code.
    pub error: String,
    /// English text for logs and people.
    pub message: String,
}

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
    /// Unavailable until a known time (maintenance): `Retry-After` in whole
    /// seconds.
    #[error("service unavailable")]
    UnavailableFor { retry_after_s: u64 },
    #[error("This licence key is blocked.")]
    KeyBlocked,
    #[error("This licence is not active: the subscription ended or the purchase was refunded.")]
    LicenceInactive,
    #[error("This licence key is not known.")]
    KeyUnknown,
    #[error("This licence is active in as many browsers as it allows.")]
    SeatLimit,
    /// Details are logged, never sent to the client.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound | Self::KeyUnknown => StatusCode::NOT_FOUND,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::Unavailable | Self::UnavailableFor { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::KeyBlocked | Self::LicenceInactive => StatusCode::FORBIDDEN,
            Self::SeatLimit => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad-request",
            Self::NotFound => "not-found",
            Self::PayloadTooLarge => "payload-too-large",
            Self::RateLimited { .. } => "rate-limited",
            Self::Unavailable | Self::UnavailableFor { .. } => "unavailable",
            Self::KeyBlocked => "key-blocked",
            Self::LicenceInactive => "licence-inactive",
            Self::KeyUnknown => "key-unknown",
            Self::SeatLimit => "seat-limit",
            Self::Internal(_) => "internal",
        }
    }
}

impl From<sqlx::Error> for AppError {
    /// A database that cannot be reached is `unavailable` (503 without
    /// `Retry-After`: nobody knows when it will be back); anything else is
    /// internal.
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::PoolTimedOut
            | sqlx::Error::PoolClosed
            | sqlx::Error::Io(_)
            | sqlx::Error::Tls(_) => {
                tracing::warn!(error = %err, "database unavailable");
                Self::Unavailable
            }
            other => Self::Internal(other.into()),
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
        if let Self::RateLimited { retry_after_s } | Self::UnavailableFor { retry_after_s } = self {
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
    fn unavailable_for_a_known_time_carries_retry_after() {
        let response = AppError::UnavailableFor { retry_after_s: 600 }.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[RETRY_AFTER], "600");
    }

    #[test]
    fn the_licence_codes_and_their_statuses() {
        for (error, status, code) in [
            (AppError::KeyBlocked, 403, "key-blocked"),
            (AppError::LicenceInactive, 403, "licence-inactive"),
            (AppError::KeyUnknown, 404, "key-unknown"),
            (AppError::SeatLimit, 409, "seat-limit"),
        ] {
            assert_eq!((error.status().as_u16(), error.code()), (status, code));
        }
    }

    #[test]
    fn unavailable_has_no_retry_after() {
        let response = AppError::Unavailable.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().get(RETRY_AFTER).is_none());
    }
}
