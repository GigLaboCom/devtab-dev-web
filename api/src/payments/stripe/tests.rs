//! The adapter against a stand-in for Stripe's API on the loopback address:
//! what it sends, and how it reads answers and errors.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde_json::{Value, json};

use super::*;

/// A request as the stand-in saw it: path, headers, form or query.
type Request = (String, HashMap<String, String>, Vec<(String, String)>);

/// What the stand-in saw.
#[derive(Debug, Default)]
struct Seen {
    requests: Vec<Request>,
    /// Answer 500 to this many requests first.
    fail_first: usize,
}

type Shared = Arc<Mutex<Seen>>;

fn headers(map: &HeaderMap) -> HashMap<String, String> {
    map.iter()
        .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
        .collect()
}

fn record(seen: &Shared, path: &str, map: &HeaderMap, form: Vec<(String, String)>) -> bool {
    let mut seen = seen.lock().unwrap();
    seen.requests.push((path.to_owned(), headers(map), form));
    if seen.fail_first > 0 {
        seen.fail_first -= 1;
        return false;
    }
    true
}

fn price(id: &str, amount: Option<i64>, recurring: &Value) -> Value {
    json!({
        "id": id, "object": "price", "active": true, "currency": "eur",
        "unit_amount": amount, "recurring": recurring,
    })
}

fn not_found(what: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(
            json!({"error": {"message": format!("No such price: '{what}'"),
                               "code": "resource_missing"}}),
        ),
    )
}

async fn price_by_id(
    State(seen): State<Shared>,
    map: HeaderMap,
    Path(id): Path<String>,
) -> (StatusCode, Json<Value>) {
    if !record(&seen, &format!("/v1/prices/{id}"), &map, Vec::new()) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({})));
    }
    match id.as_str() {
        "price_month" => (
            StatusCode::OK,
            Json(price(
                &id,
                Some(900),
                &json!({"interval": "month", "interval_count": 1}),
            )),
        ),
        "price_once" => (StatusCode::OK, Json(price(&id, Some(5000), &Value::Null))),
        "price_tiered" => (
            StatusCode::OK,
            Json(price(&id, None, &json!({"interval": "month"}))),
        ),
        _ => not_found(&id),
    }
}

async fn prices_by_lookup_key(
    State(seen): State<Shared>,
    map: HeaderMap,
    Query(query): Query<Vec<(String, String)>>,
) -> Json<Value> {
    record(&seen, "/v1/prices", &map, query.clone());
    let key = query
        .iter()
        .find(|(k, _)| k == "lookup_keys[]")
        .map(|(_, v)| v.as_str());
    let data = match key {
        Some("pro-annual") => vec![price(
            "price_year",
            Some(9000),
            &json!({"interval": "year", "interval_count": 1}),
        )],
        Some("pro-quarterly") => vec![price(
            "price_quarter",
            Some(2500),
            &json!({"interval": "month", "interval_count": 3}),
        )],
        Some("twice") => vec![
            price("price_a", Some(1), &Value::Null),
            price("price_b", Some(1), &Value::Null),
        ],
        _ => Vec::new(),
    };
    Json(json!({"object": "list", "data": data}))
}

async fn checkout(
    State(seen): State<Shared>,
    map: HeaderMap,
    Form(form): Form<Vec<(String, String)>>,
) -> (StatusCode, Json<Value>) {
    if !record(&seen, "/v1/checkout/sessions", &map, form.clone()) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": {"message": "try again"}})),
        );
    }
    let embedded = form.iter().any(|(k, _)| k == "ui_mode");
    let answer = if embedded {
        json!({"id": "cs_test_e", "client_secret": "cs_test_e_secret_x", "url": null})
    } else {
        json!({"id": "cs_test_h", "url": "https://checkout.stripe.com/c/pay/cs_test_h"})
    };
    (StatusCode::OK, Json(answer))
}

async fn portal(
    State(seen): State<Shared>,
    map: HeaderMap,
    Form(form): Form<Vec<(String, String)>>,
) -> (StatusCode, Json<Value>) {
    record(&seen, "/v1/billing_portal/sessions", &map, form);
    (
        StatusCode::FORBIDDEN,
        Json(
            json!({"error": {"message": "The provided key does not have the required permissions"}}),
        ),
    )
}

async fn stand_in(fail_first: usize) -> (String, Shared) {
    let seen: Shared = Arc::new(Mutex::new(Seen {
        fail_first,
        ..Seen::default()
    }));
    let app = Router::new()
        .route("/v1/prices", get(prices_by_lookup_key))
        .route("/v1/prices/{id}", get(price_by_id))
        .route("/v1/checkout/sessions", post(checkout))
        .route("/v1/billing_portal/sessions", post(portal))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

fn settings(api_url: &str, ui: CheckoutUi, managed: bool) -> StripeSettings {
    StripeSettings {
        mode: StripeMode::Sandbox,
        api_key_file: PathBuf::from("unused"),
        webhook_secret_file: None,
        managed_payments: managed,
        automatic_tax: !managed,
        checkout_ui: ui,
        publishable_key: Some("pk_test_public".to_owned()),
        api_url: api_url.to_owned(),
        api_version: DEFAULT_API_VERSION.to_owned(),
        timeout_s: 5,
        webhook_tolerance_s: 300,
    }
}

fn provider(api_url: &str, ui: CheckoutUi, managed: bool) -> StripeProvider {
    let key = ApiKey::parse("rk_test_key", StripeMode::Sandbox).unwrap();
    StripeProvider::with_key(settings(api_url, ui, managed), key)
        .unwrap()
        .with_retry_delays(vec![std::time::Duration::ZERO; 3])
}

fn request() -> CheckoutRequest {
    CheckoutRequest {
        checkout_id: Uuid::from_u128(1),
        price_id: "price_month".to_owned(),
        customer: None,
        locale: Some("en".to_owned()),
        success_url: "https://web.example.invalid/pro/thanks/".to_owned(),
        cancel_url: "https://web.example.invalid/pro/".to_owned(),
    }
}

fn value<'a>(form: &'a [(String, String)], key: &str) -> Option<&'a str> {
    form.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn a_price_by_id_carries_its_amount_and_billing() {
    let (url, seen) = stand_in(0).await;
    let stripe = provider(&url, CheckoutUi::Hosted, true);
    let found = stripe
        .resolve(&PriceKey::Id("price_month".to_owned()))
        .await
        .unwrap();
    assert_eq!(found.id, "price_month");
    assert_eq!(
        found.snapshot,
        PriceSnapshot {
            currency: "EUR".to_owned(),
            amount_minor: 900
        }
    );
    assert_eq!(found.billing, Billing::Every("month".to_owned()));
    let once = stripe
        .resolve(&PriceKey::Id("price_once".to_owned()))
        .await
        .unwrap();
    assert_eq!(once.billing, Billing::OneTime);
    assert_eq!(stripe.price("price_month").await.unwrap().amount_minor, 900);

    let seen = seen.lock().unwrap();
    let (_, sent, _) = &seen.requests[0];
    assert_eq!(sent["authorization"], "Bearer rk_test_key");
    assert_eq!(sent["stripe-version"], DEFAULT_API_VERSION);
    assert!(!sent.contains_key("idempotency-key"), "no key on a GET");
}

#[tokio::test]
async fn a_price_by_lookup_key_asks_for_active_prices_only() {
    let (url, seen) = stand_in(0).await;
    let stripe = provider(&url, CheckoutUi::Hosted, true);
    let found = stripe
        .resolve(&PriceKey::LookupKey("pro-annual".to_owned()))
        .await
        .unwrap();
    assert_eq!(
        (found.id.as_str(), found.billing),
        ("price_year", Billing::Every("year".to_owned()))
    );
    let quarter = stripe
        .resolve(&PriceKey::LookupKey("pro-quarterly".to_owned()))
        .await
        .unwrap();
    assert_eq!(quarter.billing, Billing::Every("3 month".to_owned()));
    let query = &seen.lock().unwrap().requests[0].2;
    assert_eq!(value(query, "lookup_keys[]"), Some("pro-annual"));
    assert_eq!(value(query, "active"), Some("true"));
}

#[tokio::test]
async fn missing_odd_and_ambiguous_prices_are_errors() {
    let (url, _) = stand_in(0).await;
    let stripe = provider(&url, CheckoutUi::Hosted, true);
    let missing = |key: PriceKey| {
        let stripe = stripe.clone();
        async move { stripe.resolve(&key).await.unwrap_err() }
    };
    assert!(matches!(
        missing(PriceKey::Id("price_nope".to_owned())).await,
        ProviderError::NotFound(m) if m.contains("resource_missing")
    ));
    assert!(matches!(
        missing(PriceKey::Id("../v1/customers".to_owned())).await,
        ProviderError::NotFound(_)
    ));
    assert!(matches!(
        missing(PriceKey::Id("price_tiered".to_owned())).await,
        ProviderError::Refused(_)
    ));
    assert!(matches!(
        missing(PriceKey::LookupKey("nope".to_owned())).await,
        ProviderError::NotFound(_)
    ));
    assert!(matches!(
        missing(PriceKey::LookupKey("twice".to_owned())).await,
        ProviderError::Refused(_)
    ));
}

#[tokio::test]
async fn a_hosted_checkout_with_managed_payments() {
    let (url, seen) = stand_in(0).await;
    let stripe = provider(&url, CheckoutUi::Hosted, true);
    let start = stripe.create_checkout(&request()).await.unwrap();
    assert_eq!(start.provider_checkout_id, "cs_test_h");
    assert_eq!(
        start.step,
        CheckoutStep::Redirect {
            url: "https://checkout.stripe.com/c/pay/cs_test_h".to_owned()
        }
    );
    let seen = seen.lock().unwrap();
    let (path, sent, form) = &seen.requests[0];
    assert_eq!(path, "/v1/checkout/sessions");
    let id = Uuid::from_u128(1).to_string();
    assert_eq!(sent["idempotency-key"], format!("checkout-{id}"));
    for (key, want) in [
        ("mode", "subscription"),
        ("line_items[0][price]", "price_month"),
        ("line_items[0][quantity]", "1"),
        ("client_reference_id", id.as_str()),
        ("metadata[devtab_checkout]", id.as_str()),
        ("subscription_data[metadata][devtab_checkout]", id.as_str()),
        ("success_url", "https://web.example.invalid/pro/thanks/"),
        ("cancel_url", "https://web.example.invalid/pro/"),
        ("locale", "en"),
        ("managed_payments[enabled]", "true"),
    ] {
        assert_eq!(value(form, key), Some(want), "{key}");
    }
    for absent in [
        "automatic_tax[enabled]",
        "ui_mode",
        "customer",
        "customer_email",
        "payment_method_types[0]",
    ] {
        assert_eq!(value(form, absent), None, "{absent}");
    }
}

#[tokio::test]
async fn an_embedded_checkout_on_the_operator_s_own_account() {
    let (url, seen) = stand_in(1).await;
    let stripe = provider(&url, CheckoutUi::Embedded, false);
    let mut request = request();
    request.customer = Some("cus_1".to_owned());
    let start = stripe.create_checkout(&request).await.unwrap();
    assert_eq!(
        start.step,
        CheckoutStep::Embedded {
            client_secret: "cs_test_e_secret_x".to_owned(),
            publishable_key: "pk_test_public".to_owned(),
            api_version: DEFAULT_API_VERSION.to_owned(),
        }
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.requests.len(), 2, "one 500, then a retry");
    assert_eq!(
        seen.requests[0].1["idempotency-key"], seen.requests[1].1["idempotency-key"],
        "the retry has the same idempotency key"
    );
    let form = &seen.requests[1].2;
    assert_eq!(value(form, "ui_mode"), Some("embedded_page"));
    assert_eq!(
        value(form, "return_url"),
        Some("https://web.example.invalid/pro/thanks/")
    );
    assert_eq!(value(form, "customer"), Some("cus_1"));
    assert_eq!(value(form, "automatic_tax[enabled]"), Some("true"));
    for absent in ["success_url", "cancel_url", "managed_payments[enabled]"] {
        assert_eq!(value(form, absent), None, "{absent}");
    }
}

#[tokio::test]
async fn retries_stop_after_the_last_pause() {
    let (url, seen) = stand_in(10).await;
    let stripe = provider(&url, CheckoutUi::Hosted, true);
    let err = stripe.create_checkout(&request()).await.unwrap_err();
    assert!(matches!(err, ProviderError::Unavailable(m) if m == "try again"));
    assert_eq!(seen.lock().unwrap().requests.len(), 4);
}

#[tokio::test]
async fn a_refusal_is_not_retried() {
    let (url, seen) = stand_in(0).await;
    let stripe = provider(&url, CheckoutUi::Hosted, true);
    let err = stripe
        .portal_url(
            "cus_1",
            "https://web.example.invalid/pro/manage/",
            Uuid::from_u128(2),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Refused(m) if m.starts_with("403")));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.requests.len(), 1);
    assert_eq!(
        seen.requests[0].1["idempotency-key"],
        format!("portal-{}", Uuid::from_u128(2))
    );
}

#[tokio::test]
async fn no_answer_is_unavailable() {
    let stripe = provider("http://127.0.0.1:9", CheckoutUi::Hosted, true);
    let err = stripe.price("price_month").await.unwrap_err();
    assert!(matches!(err, ProviderError::Unavailable(_)), "{err}");
}

#[test]
fn api_versions_are_checked() {
    assert!(is_supported_api_version(DEFAULT_API_VERSION));
    assert!(is_supported_api_version(OLDEST_API_VERSION));
    assert!(is_supported_api_version("2027-04-28.fig"));
    for old_or_odd in [
        "2026-08-26.dahlia",
        "2025-03-31.basil",
        "2026-11-18",
        "2026-11-18.",
        "2026-11-18.Endive",
        "26-11-18.endive",
        "latest",
    ] {
        assert!(!is_supported_api_version(old_or_odd), "{old_or_odd}");
    }
}
