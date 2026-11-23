//! HTTP tests: the routers and the middleware through `oneshot`, without a
//! network and without a database.
#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use devtab_api::http::json::JsonBody;
use devtab_api::{AppState, Config, Store, http, internal_app, public_app};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;

/// A state whose database is not there: fine for every route but readyz.
fn state() -> AppState {
    AppState::new(Store::connect_lazy("postgres://nobody@127.0.0.1:1/none", 1).unwrap())
}

fn public() -> Router {
    public_app(&Config::default(), state())
}

async fn json_body(res: axum::response::Response) -> Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn get_request(path: &str) -> Request<Body> {
    Request::get(path).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn healthz_is_ok() {
    let res = public().oneshot(get_request("/api/healthz")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json_body(res).await, json!({ "status": "ok" }));
}

#[tokio::test]
async fn internal_healthz_is_ok_and_only_on_the_internal_listener() {
    let internal = internal_app(&Config::default(), state());
    let res = internal
        .clone()
        .oneshot(get_request("/internal/healthz"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json_body(res).await, json!({ "status": "ok" }));

    let res = public()
        .oneshot(get_request("/internal/healthz"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = internal.oneshot(get_request("/api/healthz")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_path_is_404_in_the_error_shape() {
    let res = public()
        .oneshot(get_request("/api/nope?x=1"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        json_body(res).await,
        json!({ "error": "not-found", "message": "not found" })
    );
}

#[tokio::test]
async fn wrong_method_is_405_in_the_error_shape_with_allow() {
    let res = public()
        .oneshot(Request::post("/api/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(res.headers().contains_key("allow"));
    assert_eq!(json_body(res).await["error"], "method-not-allowed");
}

#[tokio::test]
async fn a_17_kib_body_is_413() {
    let big = vec![b'x'; 17 * 1024];
    let res = public()
        .oneshot(
            Request::post("/api/healthz")
                .header(CONTENT_LENGTH, big.len())
                .body(Body::from(big))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json_body(res).await["error"], "payload-too-large");
}

#[tokio::test]
async fn readyz_without_a_database_is_503() {
    let res = public().oneshot(get_request("/api/readyz")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(res.headers().get("retry-after").is_none());
    assert_eq!(
        json_body(res).await,
        json!({ "error": "unavailable", "message": "service unavailable" })
    );
}

#[tokio::test]
async fn request_id_is_generated_when_absent() {
    let res = public().oneshot(get_request("/api/healthz")).await.unwrap();
    let id = res.headers().get("x-request-id").expect("x-request-id");
    assert_eq!(id.to_str().unwrap().len(), 36, "a UUID");
}

#[tokio::test]
async fn request_id_is_returned_when_present() {
    let res = public()
        .oneshot(
            Request::get("/api/healthz")
                .header("x-request-id", "abc-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.headers()["x-request-id"], "abc-123");
}

/// The standard layers around routes that read a body or panic.
fn test_router() -> Router {
    http::with_layers(
        Router::new()
            .route(
                "/echo",
                post(|body: Bytes| async move { body.len().to_string() }),
            )
            .route(
                "/json",
                post(|JsonBody(value): JsonBody<Value>| async move { value.to_string() }),
            )
            .route(
                "/panic",
                get(|| async {
                    panic!("boom");
                    #[allow(unreachable_code)]
                    ""
                }),
            ),
    )
}

#[tokio::test]
async fn a_streamed_body_over_the_limit_is_413_and_one_at_the_limit_passes() {
    let res = test_router()
        .oneshot(
            Request::post("/echo")
                .body(Body::from(vec![b'x'; http::BODY_LIMIT + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json_body(res).await["error"], "payload-too-large");

    let res = test_router()
        .oneshot(
            Request::post("/echo")
                .body(Body::from(vec![b'x'; http::BODY_LIMIT]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn json_rejections_use_the_error_shape() {
    let res = test_router()
        .oneshot(Request::post("/json").body(Body::from("{}")).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(res).await,
        json!({ "error": "bad-request", "message": "expected Content-Type: application/json" })
    );

    let res = test_router()
        .oneshot(
            Request::post("/json")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(res).await["error"], "bad-request");
}

#[tokio::test]
async fn a_panic_is_500_in_the_error_shape() {
    let res = test_router().oneshot(get_request("/panic")).await.unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        json_body(res).await,
        json!({ "error": "internal", "message": "internal error" })
    );
}
