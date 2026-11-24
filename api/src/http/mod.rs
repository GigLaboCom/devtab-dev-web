//! HTTP routers and the middleware stack.

pub mod client_ip;
pub mod cors;
pub mod error;
pub mod health;
pub mod internal;
pub mod json;
pub mod plans;
pub mod rate_limit;
pub mod v1;

use std::any::Any;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderName, HeaderValue, Request, Response, StatusCode};
use axum::middleware::{self, Next};
use axum::routing::get;
use error::{AppError, error_body, error_json};
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{
    MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::config::Config;
use crate::state::AppState;

pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");
pub const BODY_LIMIT: usize = 16 * 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

/// Routes of the public listener, without the standard middleware: the
/// health checks and the extension's endpoints (licence and plans).
pub fn public_routes(config: &Config, state: AppState) -> Router {
    Router::new()
        .route("/api/healthz", get(health::healthz))
        .route("/api/readyz", get(health::readyz))
        .with_state(state.clone())
        .merge(v1::routes(config, state.clone()))
        .merge(plans::v1_routes(config, state))
        .fallback(|| async { AppError::NotFound })
}

/// Routes of the internal listener, without middleware.
pub fn internal_routes(config: &Config, state: AppState) -> Router {
    internal::routes(config, state)
}

/// Wraps a router in the standard middleware stack, outermost first:
/// request id → trace → propagate request id → error shape → body limit →
/// timeout → catch panic.
pub fn with_layers(router: Router) -> Router {
    let trace = TraceLayer::new_for_http()
        .make_span_with(|req: &Request<Body>| {
            let request_id = req
                .extensions()
                .get::<RequestId>()
                .and_then(|id| id.header_value().to_str().ok())
                .unwrap_or("-");
            tracing::info_span!(
                "http",
                method = %req.method(),
                path = %req.uri().path(),
                request_id = %request_id,
            )
        })
        .on_response(DefaultOnResponse::new().level(Level::INFO));

    router.layer(
        ServiceBuilder::new()
            .layer(SetRequestIdLayer::new(REQUEST_ID_HEADER, MakeRequestUuid))
            .layer(trace)
            .layer(PropagateRequestIdLayer::new(REQUEST_ID_HEADER))
            .layer(middleware::from_fn(error_shape))
            .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
            .layer(TimeoutLayer::with_status_code(
                StatusCode::SERVICE_UNAVAILABLE,
                REQUEST_TIMEOUT,
            ))
            .layer(CatchPanicLayer::custom(panic_response)),
    )
}

/// Gives the bodiless or plain-text answers of the router and the layers
/// (an oversized body, the timeout, a wrong method) the API's error shape.
async fn error_shape(request: Request<Body>, next: Next) -> Response<Body> {
    let response = next.run(request).await;
    let (code, message) = match response.status() {
        StatusCode::PAYLOAD_TOO_LARGE => ("payload-too-large", "request body is too large"),
        StatusCode::SERVICE_UNAVAILABLE => ("unavailable", "service unavailable"),
        StatusCode::METHOD_NOT_ALLOWED => ("method-not-allowed", "method not allowed"),
        _ => return response,
    };
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    if is_json {
        return response;
    }
    let (mut parts, _) = response.into_parts();
    parts.headers.remove(CONTENT_LENGTH);
    parts
        .headers
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Response::from_parts(parts, Body::from(error_json(code, message).to_string()))
}

#[allow(clippy::needless_pass_by_value)] // the signature `ResponseForPanic` requires
fn panic_response(err: Box<dyn Any + Send + 'static>) -> Response<Body> {
    let detail = err
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| err.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic");
    tracing::error!(panic = detail, "handler panicked");
    error_body(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "internal error",
    )
}
