//! The plans' endpoints: `GET /v1/plans` on the public listener for the
//! extension, `GET /internal/plans` on the internal one for the site's pages.
//!
//! `/v1/plans` takes nothing about the person asking: no body, no parameter,
//! no cookie. Its answer is public and the same for everyone, so any origin
//! may read it (`Access-Control-Allow-Origin: *`, never credentials, on errors
//! too). It carries `Cache-Control: no-cache` and an `ETag`, and answers 304 to
//! an `If-None-Match` with that tag: a cache on the way may keep it but
//! revalidates every time, so none adds its age to the catalogue's `maxAge`.
//! Like the other `/v1/*` endpoints it answers only on the licence host and
//! is closed during maintenance; its rate limit is per client address, a
//! minute.

use axum::Router;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, Request, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_MAX_AGE,
    CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::{IntoResponse as _, Response};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use tower::ServiceBuilder;
use tower_http::limit::RequestBodyLimitLayer;
use utoipa::{IntoParams, ToSchema};

use super::error::AppError;
use super::{rate_limit, v1};
use crate::config::Config;
use crate::plans::MAX_ANSWER_BYTES;
use crate::plans::catalogue::{self, CatalogueRows};
use crate::state::AppState;
use crate::store::plans as store;

/// The stored catalogue, read in one snapshot: the plans, their texts, the
/// active prices of the current provider and the version.
async fn load(state: &AppState) -> Result<CatalogueRows, AppError> {
    let mut tx = state.store.pool().begin().await?;
    sqlx::query("set transaction isolation level repeatable read, read only")
        .execute(&mut *tx)
        .await?;
    let prices = match state.catalogue.provider.as_deref() {
        Some(provider) => store::active_prices(&mut *tx, provider).await?,
        None => Vec::new(),
    };
    let rows = CatalogueRows {
        version: store::catalog_version(&mut *tx).await?,
        plans: store::all(&mut *tx).await?,
        texts: store::texts(&mut *tx).await?,
        prices,
    };
    tx.commit().await?;
    Ok(rows)
}

/// A JSON answer with `Cache-Control: no-cache` and its `ETag`, or 304 when
/// the request's `If-None-Match` names that tag.
fn answer(
    value: &impl Serialize,
    headers: &HeaderMap,
    limit: Option<usize>,
) -> Result<Response, AppError> {
    let body = serde_json::to_vec(value).map_err(anyhow::Error::from)?;
    if let Some(limit) = limit
        && body.len() > limit
    {
        // plans sync refuses such a catalogue; never send what the extension
        // would not read.
        tracing::error!(
            bytes = body.len(),
            limit,
            "the plans' answer is over its size limit"
        );
        return Err(AppError::Unavailable);
    }
    let tag = catalogue::etag(&body);
    let matches = headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| catalogue::none_match(value, &tag));
    let mut response = if matches {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut response = body.into_response();
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        response
    };
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Ok(value) = HeaderValue::from_str(&tag) {
        headers.insert(ETAG, value);
    }
    Ok(response)
}

/// `GET /v1/plans`: the catalogue for the extension.
async fn v1_plans(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, AppError> {
    let rows = load(&state).await?;
    let body = catalogue::public(&rows, &state.catalogue, state.clock.now());
    answer(&body, &headers, Some(MAX_ANSWER_BYTES))
}

/// Any origin may read the plans, never with credentials; a preflight gets
/// 204 with `GET` and a day's max age.
async fn plans_cors(request: Request, next: Next) -> Response {
    let mut response = if request.method() == Method::OPTIONS {
        let mut response = StatusCode::NO_CONTENT.into_response();
        let headers = response.headers_mut();
        headers.insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET"),
        );
        headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("86400"));
        response
    } else {
        next.run(request).await
    };
    response
        .headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
}

/// `/v1/plans` with its guards: the licence host, CORS, maintenance, the body
/// limit of `/v1/*` and the client address's rate limit.
pub fn v1_routes(config: &Config, state: AppState) -> Router {
    let guard = v1::Guard::new(config, &state);
    let per_ip = state.limits.plans_per_ip.clone();
    Router::new()
        .route(
            "/v1/plans",
            get(v1_plans).layer(from_fn_with_state(per_ip, rate_limit::per_ip)),
        )
        .layer(
            ServiceBuilder::new()
                .layer(from_fn_with_state(guard.clone(), v1::licence_host))
                .layer(from_fn(plans_cors))
                .layer(from_fn_with_state(guard, v1::maintenance))
                .layer(RequestBodyLimitLayer::new(v1::BODY_LIMIT)),
        )
        .with_state(state)
}

/// The languages of the site's pages.
#[derive(Debug, Clone, Copy, Default, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    #[default]
    En,
    Ru,
}

impl Locale {
    fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ru => "ru",
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PlansQuery {
    /// The language of the plans' texts; `en` when absent.
    locale: Option<Locale>,
}

/// `GET /internal/plans`: the catalogue for the site's pages.
#[utoipa::path(
    get,
    path = "/internal/plans",
    tag = "plans",
    params(PlansQuery),
    responses(
        (status = 200, description = "Every plan whose offer has not ended, in the order of the cards, \
          with the texts of the language asked for; `available` says which can be bought now", body = catalogue::SiteCatalogue),
        (status = 304, description = "Not modified: the `If-None-Match` names the current `ETag`"),
        (status = 400, description = "An unknown language", body = super::error::ErrorBody),
        (status = 503, description = "The database cannot be reached", body = super::error::ErrorBody),
    ),
)]
pub async fn internal_plans(
    State(state): State<AppState>,
    query: Result<Query<PlansQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let Query(query) =
        query.map_err(|_| AppError::BadRequest("locale is `en` or `ru`".to_owned()))?;
    let locale = query.locale.unwrap_or_default().code();
    let rows = load(&state).await?;
    let body = catalogue::site(&rows, &state.catalogue, locale, state.clock.now());
    answer(&body, &headers, None)
}
