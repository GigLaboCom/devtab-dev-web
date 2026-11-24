//! The internal listener, for the site's own pages and never published:
//! its routes, and their description as an `OpenAPI` document, from which `script/gen` writes
//! `contract/internal-openapi.json` and the site's types
//! (`src/lib/api/schema.gen.ts`).

use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::{OpenApi, ToSchema};

use super::error::{AppError, ErrorBody};
use super::plans;
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

/// Routes of the internal listener, without middleware.
pub fn routes(_config: &Config, state: AppState) -> Router {
    Router::new()
        .route("/internal/healthz", get(healthz))
        .route("/internal/plans", get(plans::internal_plans))
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
    paths(healthz, plans::internal_plans),
    components(schemas(
        Health,
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
        assert_eq!(paths, ["/internal/healthz", "/internal/plans"]);
        assert!(doc["components"]["schemas"]["SiteCatalogue"].is_object());
        assert!(doc["info"].get("license").is_none());
    }
}
