//! `POST /internal/trials`: the signed-in person starts their trial without a
//! card (`crate::trial`), where the deployment offers one.

use axum::Json;
use axum::extract::State;
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::{AppError, ErrorBody};
use super::json::JsonBody;
use super::plans::Locale;
use crate::auth::CurrentUser;
use crate::clock::rfc3339;
use crate::state::AppState;

/// A trial to start.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTrial {
    /// The language of the trial's mails.
    #[serde(default)]
    pub locale: Locale,
}

/// A started trial.
#[derive(Debug, Serialize, ToSchema)]
pub struct StartedTrial {
    /// The licence's id.
    pub lid: String,
    /// The licence key, shown only in this answer.
    pub key: String,
    /// When the trial ends, RFC 3339.
    pub ends_at: String,
}

/// `POST /internal/trials`.
#[utoipa::path(
    post,
    path = "/internal/trials",
    operation_id = "start_trial",
    tag = "purchase",
    security(("bearer" = [])),
    request_body = NewTrial,
    responses(
        (status = 201, description = "Started", body = StartedTrial),
        (status = 401, description = "No access token, or one that does not pass", body = ErrorBody),
        (status = 403, description = "`trial-off`: no trial is offered here", body = ErrorBody),
        (status = 409, description = "`trial-used`: this inbox had one (any spelling, any account); \
          `trial-not-eligible`: the account had a paid licence", body = ErrorBody),
    ),
)]
pub async fn start(
    State(state): State<AppState>,
    user: CurrentUser,
    JsonBody(body): JsonBody<NewTrial>,
) -> Result<Response, AppError> {
    let settings = state.trial.as_ref().ok_or(AppError::TrialOff)?;
    let mut tx = state.store.pool().begin().await?;
    let started = crate::trial::start(
        &mut tx,
        settings,
        &state.mail_schedule,
        user.id,
        body.locale.code(),
        state.clock.now(),
    )
    .await?;
    tx.commit().await?;
    state.wake.notify_one();
    let mut response = (
        StatusCode::CREATED,
        Json(StartedTrial {
            lid: started.lid,
            key: started.key,
            ends_at: rfc3339(started.ends_at),
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}
