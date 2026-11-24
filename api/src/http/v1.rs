//! The extension's licence endpoints on the public listener: `POST
//! /v1/activate`, `/v1/refresh`, `/v1/deactivate` (`contract/licence-v1/`).
//!
//! Around them, outermost first: the host check (`DEVTAB_LICENCE_HOST`),
//! CORS, `Cache-Control: no-store`, maintenance (`DEVTAB_MAINTENANCE_UNTIL`),
//! a 4 KiB body limit and, per endpoint, the client address's rate limit.
//! Logs carry the licence id and the first four characters of the
//! installation id; never a key, a secret, a token or an address.

use axum::extract::{Request, State};
use axum::http::header::{CACHE_CONTROL, HOST};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tower::ServiceBuilder;
use tower_http::limit::RequestBodyLimitLayer;

use super::cors::{self, AllowedOrigins};
use super::error::AppError;
use super::json::JsonBody;
use super::rate_limit;
use crate::clock::SharedClock;
use crate::config::Config;
use crate::licence::wire::{self, short};
use crate::licence::{activate, refresh, seats};
use crate::state::AppState;

/// The largest request body of `/v1/*`.
pub const BODY_LIMIT: usize = 4 * 1024;

fn contract_breach() -> AppError {
    AppError::BadRequest("The request does not follow the contract.".to_owned())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivateBody {
    key: String,
    inst: String,
}

#[derive(Serialize)]
struct ActivateAnswer {
    token: String,
    secret: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallationBody {
    lid: String,
    inst: String,
    secret: String,
}

impl InstallationBody {
    fn checked(self) -> Result<Self, AppError> {
        if wire::is_lid(&self.lid) && wire::is_inst(&self.inst) && wire::is_secret(&self.secret) {
            Ok(self)
        } else {
            Err(contract_breach())
        }
    }
}

#[derive(Serialize)]
struct TokenAnswer {
    token: String,
}

async fn activate(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<ActivateBody>,
) -> Result<Json<ActivateAnswer>, AppError> {
    if !wire::is_key_text(&body.key) || !wire::is_inst(&body.inst) {
        return Err(contract_breach());
    }
    let inst = short(&body.inst);
    let result = async {
        let key_hash = activate::key_hash(&body.key)?;
        rate_limit::check(&state.limits.activate_per_key, &key_hash)?;
        activate::activate(&state, &key_hash, &body.inst).await
    }
    .await;
    match result {
        Ok(done) => {
            tracing::info!(lid = %done.lid, inst, outcome = "activated", "activate");
            Ok(Json(ActivateAnswer {
                token: done.token,
                secret: done.secret,
            }))
        }
        Err(err) => {
            tracing::info!(inst, outcome = err.code(), "activate refused");
            Err(err)
        }
    }
}

async fn refresh(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<InstallationBody>,
) -> Result<Json<TokenAnswer>, AppError> {
    let body = body.checked()?;
    rate_limit::check(
        &state.limits.refresh_per_installation,
        &(body.lid.clone(), body.inst.clone()),
    )?;
    let token = refresh::refresh(&state, &body.lid, &body.inst, &body.secret).await?;
    tracing::info!(lid = %body.lid, inst = short(&body.inst), "refresh");
    Ok(Json(TokenAnswer { token }))
}

async fn deactivate(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<InstallationBody>,
) -> Result<StatusCode, AppError> {
    let body = body.checked()?;
    rate_limit::check(
        &state.limits.deactivate_per_installation,
        &(body.lid.clone(), body.inst.clone()),
    )?;
    let now = state.clock.now();
    let mut tx = state.store.pool().begin().await?;
    let done = seats::deactivate(&mut tx, &body.lid, &body.inst, &body.secret, now).await?;
    tx.commit().await?;
    tracing::info!(lid = %body.lid, inst = short(&body.inst), deactivated = done, "deactivate");
    Ok(StatusCode::NO_CONTENT)
}

/// What the guards around the endpoints read.
#[derive(Debug, Clone)]
struct Guard {
    licence_host: Option<String>,
    maintenance_until: Option<OffsetDateTime>,
    clock: SharedClock,
}

/// `/v1/*` exists only on the licence host, when one is configured.
async fn licence_host(
    State(guard): State<Guard>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    if let Some(expected) = &guard.licence_host {
        let host = request
            .headers()
            .get(HOST)
            .and_then(|h| h.to_str().ok())
            .or_else(|| request.uri().host())
            .unwrap_or_default();
        let name = host.rsplit_once(':').map_or(host, |(name, _port)| name);
        if !name.eq_ignore_ascii_case(expected) {
            return Err(AppError::NotFound);
        }
    }
    Ok(next.run(request).await)
}

async fn maintenance(
    State(guard): State<Guard>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    if let Some(until) = guard.maintenance_until {
        let left = until - guard.clock.now();
        if left > time::Duration::ZERO {
            let seconds = u64::try_from(left.whole_seconds()).unwrap_or(0)
                + u64::from(left.subsec_nanoseconds() > 0);
            return Err(AppError::UnavailableFor {
                retry_after_s: seconds.max(1),
            });
        }
    }
    Ok(next.run(request).await)
}

async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The `/v1/*` routes with their guards.
pub fn routes(config: &Config, state: AppState) -> Router {
    let guard = Guard {
        licence_host: config.licence_host.clone(),
        maintenance_until: config.maintenance_until,
        clock: state.clock.clone(),
    };
    let origins = AllowedOrigins::new(&config.extension_origins);
    let activate_per_ip = state.limits.activate_per_ip.clone();
    let refresh_per_ip = state.limits.refresh_per_ip.clone();
    Router::new()
        .route(
            "/v1/activate",
            post(activate).layer(from_fn_with_state(activate_per_ip, rate_limit::per_ip)),
        )
        .route(
            "/v1/refresh",
            post(refresh).layer(from_fn_with_state(refresh_per_ip, rate_limit::per_ip)),
        )
        .route("/v1/deactivate", post(deactivate))
        .layer(
            ServiceBuilder::new()
                .layer(from_fn_with_state(guard.clone(), licence_host))
                .layer(from_fn_with_state(origins, cors::licence_cors))
                .layer(from_fn(no_store))
                .layer(from_fn_with_state(guard, maintenance))
                .layer(RequestBodyLimitLayer::new(BODY_LIMIT)),
        )
        .with_state(state)
}
