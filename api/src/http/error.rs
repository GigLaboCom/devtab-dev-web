//! The API's error type and its JSON shape:
//! `{"error":"<kebab-case code>","message":"<English text>"}`. The codes of
//! the extension's endpoints are the contract's (`contract/licence-v1/errors.json`).

use axum::Json;
use axum::http::header::{RETRY_AFTER, WWW_AUTHENTICATE};
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
    /// No signed-in person, or a token that does not pass: answered with
    /// `WWW-Authenticate: Bearer`.
    #[error("sign-in required")]
    Unauthorized,
    /// A signed-in person who may not do this (a blocked account).
    #[error("{0}")]
    Forbidden(String),
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
    /// A webhook whose signature does not match; nothing about it is logged
    /// but the provider.
    #[error("the signature does not match")]
    Signature,
    #[error("no such plan")]
    PlanUnknown,
    #[error("this plan cannot be bought now")]
    PlanUnavailable,
    /// The signed-in person has no licence that can be activated.
    #[error("You have no active licence to connect.")]
    NoLicence,
    /// Members are for a team licence only.
    #[error("This licence is not a team's.")]
    NotATeam,
    /// Every member the team bought is invited.
    #[error("Every member of this team is invited already.")]
    TeamFull,
    /// No payment provider is configured, or it could not start a checkout.
    #[error("payments are unavailable")]
    PaymentsUnavailable,
    /// This deployment does not sell without an account
    /// (`DEVTAB_GUEST_CHECKOUT`).
    #[error("Buying without an account is not offered here: sign in to buy.")]
    GuestCheckoutOff,
    /// Guest checkout is on, but the API cannot invite buyers (no Supabase
    /// secret key).
    #[error("Buying without an account is not available now: sign in to buy.")]
    GuestCheckoutUnavailable,
    /// This deployment offers no trial.
    #[error("No trial is offered here.")]
    TrialOff,
    /// This inbox had a trial already (any spelling, any account).
    #[error("This e-mail inbox has had a trial already.")]
    TrialUsed,
    /// The account had a paid licence, or has no address.
    #[error("A trial is for those who have not bought a licence.")]
    TrialNotEligible,
    /// Details are logged, never sent to the client.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) | Self::Signature => StatusCode::BAD_REQUEST,
            Self::NotFound | Self::KeyUnknown | Self::PlanUnknown | Self::NoLicence => {
                StatusCode::NOT_FOUND
            }
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::Unavailable
            | Self::UnavailableFor { .. }
            | Self::PaymentsUnavailable
            | Self::GuestCheckoutUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Forbidden(_)
            | Self::KeyBlocked
            | Self::LicenceInactive
            | Self::GuestCheckoutOff
            | Self::TrialOff => StatusCode::FORBIDDEN,
            Self::SeatLimit
            | Self::PlanUnavailable
            | Self::NotATeam
            | Self::TeamFull
            | Self::TrialUsed
            | Self::TrialNotEligible => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad-request",
            Self::NotFound => "not-found",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::PayloadTooLarge => "payload-too-large",
            Self::RateLimited { .. } => "rate-limited",
            Self::Unavailable | Self::UnavailableFor { .. } => "unavailable",
            Self::KeyBlocked => "key-blocked",
            Self::LicenceInactive => "licence-inactive",
            Self::KeyUnknown => "key-unknown",
            Self::SeatLimit => "seat-limit",
            Self::Signature => "signature",
            Self::PlanUnknown => "plan-unknown",
            Self::PlanUnavailable => "plan-unavailable",
            Self::PaymentsUnavailable => "payments-unavailable",
            Self::GuestCheckoutOff => "guest-checkout-off",
            Self::GuestCheckoutUnavailable => "guest-checkout-unavailable",
            Self::TrialOff => "trial-off",
            Self::TrialUsed => "trial-used",
            Self::TrialNotEligible => "trial-not-eligible",
            Self::NoLicence => "no-licence",
            Self::NotATeam => "not-a-team",
            Self::TeamFull => "team-full",
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
        if matches!(self, Self::Unauthorized) {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
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
    fn unauthorized_asks_for_a_bearer_token_and_forbidden_is_403() {
        let response = AppError::Unauthorized.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[WWW_AUTHENTICATE], "Bearer");
        let error = AppError::Forbidden("account blocked".to_owned());
        assert_eq!((error.status().as_u16(), error.code()), (403, "forbidden"));
    }

    #[test]
    fn a_guest_checkout_that_is_off_or_cannot_invite_says_which() {
        let off = AppError::GuestCheckoutOff;
        assert_eq!(
            (off.status().as_u16(), off.code()),
            (403, "guest-checkout-off")
        );
        let keyless = AppError::GuestCheckoutUnavailable;
        assert_eq!(
            (keyless.status().as_u16(), keyless.code()),
            (503, "guest-checkout-unavailable")
        );
    }

    #[test]
    fn a_trial_that_is_off_used_or_not_for_this_person_says_which() {
        for (error, status, code) in [
            (AppError::TrialOff, 403, "trial-off"),
            (AppError::TrialUsed, 409, "trial-used"),
            (AppError::TrialNotEligible, 409, "trial-not-eligible"),
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
