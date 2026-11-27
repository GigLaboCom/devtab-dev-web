//! `POST /internal/licences/connect`: the site connects the signed-in
//! person's extension without a key. The extension opens the site's connect
//! page with its installation id; the site, for the signed-in person, asks
//! for an activation of their licence and hands the answer to the extension.
//! It is the activation `POST /v1/activate` makes with a key (the same seat,
//! secret and token), the licence found by the account instead.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use axum::response::{IntoResponse as _, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::{AppError, ErrorBody};
use super::json::JsonBody;
use crate::auth::CurrentUser;
use crate::licence::{activate, wire};
use crate::state::AppState;

/// The installation to connect.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectRequest {
    /// The extension's installation id: 22 base64url characters.
    pub inst: String,
    /// Which licence; without it the API picks (the one this installation is
    /// on, else the longest-running with a free seat).
    pub lid: Option<String>,
}

/// What the extension stores, as after `POST /v1/activate`.
#[derive(Serialize, ToSchema)]
pub struct Connected {
    pub lid: String,
    /// The signed licence token.
    pub token: String,
    /// The installation's secret; shown to nobody but the extension.
    pub secret: String,
}

/// `POST /internal/licences/connect`.
#[utoipa::path(
    post,
    path = "/internal/licences/connect",
    tag = "licences",
    security(("bearer" = [])),
    request_body = ConnectRequest,
    responses(
        (status = 200, description = "Activated for the installation", body = Connected),
        (status = 400, description = "Not an installation id or a licence id", body = ErrorBody),
        (status = 401, description = "No access token, or one that does not pass", body = ErrorBody),
        (status = 403, description = "`key-blocked` or `licence-inactive`", body = ErrorBody),
        (status = 404, description = "`no-licence`, or `not-found` for a licence that is not theirs", body = ErrorBody),
        (status = 409, description = "`seat-limit`: every usable licence is full", body = ErrorBody),
    ),
)]
pub async fn connect(
    State(state): State<AppState>,
    user: CurrentUser,
    JsonBody(body): JsonBody<ConnectRequest>,
) -> Result<Response, AppError> {
    if !wire::is_inst(&body.inst) || body.lid.as_deref().is_some_and(|lid| !wire::is_lid(lid)) {
        return Err(AppError::BadRequest(
            "inst must be an installation id and lid a licence id".to_owned(),
        ));
    }
    let done = activate::activate_for_user(&state, user.id, &body.inst, body.lid.as_deref()).await;
    let inst = &body.inst[..4];
    let done = match done {
        Ok(done) => {
            tracing::info!(lid = %done.lid, inst, outcome = "activated", "connect");
            done
        }
        Err(err) => {
            tracing::info!(inst, outcome = err.code(), "connect refused");
            return Err(err);
        }
    };
    let mut response = Json(Connected {
        lid: done.lid,
        token: done.token,
        secret: done.secret,
    })
    .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}
