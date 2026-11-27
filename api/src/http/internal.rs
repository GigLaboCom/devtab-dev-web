//! The internal listener, for the site's own pages and never published:
//! its routes, and their description as an `OpenAPI` document, from which `script/gen` writes
//! `contract/internal-openapi.json` and the site's types
//! (`src/lib/api/schema.gen.ts`). A route for a signed-in person takes the
//! [`CurrentUser`]: the site passes the person's Supabase access token as
//! `Authorization: Bearer <token>`.

use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use axum::response::{IntoResponse as _, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};
use uuid::Uuid;

use super::error::{AppError, ErrorBody};
use super::plans;
use crate::auth::CurrentUser;
use crate::config::Config;
use crate::plans::catalogue::{Plan, PlanKind, Price, SiteCatalogue, SitePlan, Term};
use crate::state::AppState;

/// The process is up.
#[derive(Debug, Serialize, ToSchema)]
pub struct Health {
    /// Always `ok`.
    pub status: &'static str,
}

/// `GET /internal/healthz`.
#[utoipa::path(
    get,
    path = "/internal/healthz",
    tag = "health",
    responses((status = 200, description = "The process is up", body = Health)),
)]
pub async fn healthz() -> Json<Health> {
    Json(Health { status: "ok" })
}

/// The signed-in person, as the API knows them.
#[derive(Debug, Serialize, ToSchema)]
pub struct Me {
    /// The person's id: the access token's `sub`.
    #[schema(value_type = String, format = "uuid")]
    pub user: Uuid,
    /// The account's e-mail address, from Supabase Auth.
    #[schema(required = true)]
    pub email: Option<String>,
}

/// `GET /internal/me`: who the access token says is signed in. Never cached.
#[utoipa::path(
    get,
    path = "/internal/me",
    tag = "account",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The signed-in person", body = Me),
        (status = 401, description = "No access token, or one that does not pass", body = ErrorBody),
        (status = 403, description = "The account is blocked", body = ErrorBody),
        (status = 503, description = "The project's signing keys or the database cannot be reached, \
          or sign-in is not configured", body = ErrorBody),
    ),
)]
pub async fn me(State(state): State<AppState>, user: CurrentUser) -> Result<Response, AppError> {
    let email = crate::store::users::email_of(state.store.pool(), user.id).await?;
    let mut response = Json(Me {
        user: user.id,
        email,
    })
    .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// The bearer scheme of the routes for signed-in people.
struct BearerScheme;

impl Modify for BearerScheme {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
    }
}

/// Routes of the internal listener, without middleware.
pub fn routes(_config: &Config, state: AppState) -> Router {
    Router::new()
        .route("/internal/healthz", get(healthz))
        .route("/internal/plans", get(plans::internal_plans))
        .route("/internal/me", get(me))
        .fallback(|| async { AppError::NotFound })
        .with_state(state)
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "devtab-api internal",
        description = "The internal listener of devtab-api, for the site's own pages only. Every \
                       error answers {\"error\": \"<kebab-case code>\", \"message\": \"<English text>\"}."
    ),
    paths(healthz, plans::internal_plans, me),
    modifiers(&BearerScheme),
    components(schemas(
        Health,
        Me,
        ErrorBody,
        SiteCatalogue,
        SitePlan,
        Plan,
        PlanKind,
        Term,
        Price,
        plans::Locale
    ))
)]
struct InternalApi;

/// The internal listener's description, an `OpenAPI` document.
pub fn openapi() -> utoipa::openapi::OpenApi {
    let mut doc = InternalApi::openapi();
    // utoipa copies the crate's licence, and none is chosen yet.
    doc.info.license = None;
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_names_every_internal_route() {
        let doc = serde_json::to_value(openapi()).unwrap();
        let paths: Vec<&String> = doc["paths"].as_object().unwrap().keys().collect();
        assert_eq!(
            paths,
            ["/internal/healthz", "/internal/me", "/internal/plans"]
        );
        assert!(doc["components"]["securitySchemes"]["bearer"].is_object());
        assert!(doc["components"]["schemas"]["SiteCatalogue"].is_object());
        assert!(doc["info"].get("license").is_none());
    }
}
