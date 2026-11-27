//! The plans' catalogue: `admin plans sync` against the local Supabase
//! database (`DEVTAB_TEST_DATABASE_URL`, as the API role, in transactions that
//! are rolled back), and `GET /v1/plans` / `GET /internal/plans` through the
//! routers (`oneshot`, no network) — with the database for the answers, and
//! without it for the guards around them.
//!
//! `server_catalogs` builds the answers of `contract/plans-v1/server-catalogs.json`
//! from the fixture catalogue; `pnpm test:contract` runs them through the
//! extension's own `parseCatalog`. After a change to the catalogue's format,
//! `DEVTAB_WRITE_VECTORS=1 cargo test -p devtab-api --test plans server_catalogs`
//! writes the file again.
//!
//! The catalogue is one for the whole database, so the database tests of this
//! file take turns. Those that need committed rows (the HTTP answers) use
//! plans named `test-cat-*`, sorted before any other, and remove them again.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod support;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use axum::Router;
use axum::body::Body;
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_MAX_AGE, ALLOW, CACHE_CONTROL, CONTENT_TYPE, ETAG, HOST, IF_NONE_MATCH, ORIGIN,
    RETRY_AFTER,
};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use devtab_api::admin::Context;
use devtab_api::clock::{ManualClock, parse_rfc3339};
use devtab_api::config::RateLimits;
use devtab_api::payments::{PaymentProvider, PriceSnapshot, ProviderError, ProviderFuture};
use devtab_api::plans::catalogue::{self, CatalogueRows, CatalogueSettings};
use devtab_api::plans::{file, sync};
use devtab_api::store::plans::{self as store, NewPrice, PlanRow, PlanTextRow, PriceRow};
use devtab_api::{Config, internal_app, public_app};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use sqlx::{Connection as _, PgConnection};
use support::schema::problems;
use time::OffsetDateTime;
use tower::ServiceExt as _;

const SCHEMA: &str = "plans-v1/plans.response.schema.json";
const VECTORS: &str = "contract/plans-v1/server-catalogs.json";
const ORIGIN_OF_A_PAGE: &str = "https://elsewhere.example.invalid";

/// The database tests take turns: the catalogue is one for the database.
static CATALOGUE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn fixture() -> PathBuf {
    support::repo("api/tests/fixtures/plans/three-plans.toml")
}

fn at(text: &str) -> OffsetDateTime {
    parse_rfc3339(text).unwrap()
}

/// The moment the vectors are built at.
fn vectors_now() -> OffsetDateTime {
    at("2026-11-24T00:00:00Z")
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp()).unwrap()
}

/// The rows a sync of `path` would store, read without a database.
async fn rows_of(path: &Path, version: &str) -> CatalogueRows {
    let plans_file = file::read(path).unwrap();
    let prices = sync::resolve_prices(&plans_file, &[]).await.unwrap();
    let mut plans: Vec<PlanRow> = plans_file.plans.iter().map(|e| e.plan.clone()).collect();
    plans.sort_by(|a, b| (a.sort, &a.id).cmp(&(b.sort, &b.id)));
    CatalogueRows {
        version: Some(version.to_owned()),
        plans,
        texts: plans_file
            .plans
            .iter()
            .flat_map(|e| e.texts.iter().cloned())
            .collect(),
        prices: prices
            .into_iter()
            .map(|p| PriceRow {
                id: uuid::Uuid::nil(),
                plan_id: p.plan_id,
                provider: p.provider,
                provider_price_id: p.provider_price_id,
                currency: p.currency,
                amount_minor: p.amount_minor,
                active: true,
            })
            .collect(),
    }
}

fn staging() -> CatalogueSettings {
    CatalogueSettings {
        test_currencies: false,
        ..CatalogueSettings::default()
    }
}

/// The cases of the vectors: the catalogue, the environment, and what the
/// extension makes of the answer — a test or development build (XTS among
/// its currencies) and a production build (Intl's currencies only).
async fn cases() -> Vec<(
    &'static str,
    &'static str,
    CatalogueRows,
    CatalogueSettings,
    Value,
)> {
    let three = rows_of(&fixture(), "2026-11-24.1").await;
    let mut offer = three.clone();
    offer.plans[2].until = Some(at("2027-01-01T00:00:00Z"));
    let repository = rows_of(&support::repo("config/plans.toml"), "2026-11-24.1").await;
    let all_kept =
        json!({ "ok": true, "kept": ["free", "pro-monthly", "pro-annual"], "dropped": [] });
    let paid_dropped = json!({
        "ok": true,
        "kept": ["free"],
        "dropped": [{ "index": 1, "why": "currency" }, { "index": 2, "why": "currency" }],
    });
    let free_only = json!({ "ok": true, "kept": ["free"], "dropped": [] });
    let empty = json!({ "ok": false, "why": "empty" });
    vec![
        (
            "three-plans",
            "dev",
            three.clone(),
            CatalogueSettings::default(),
            json!({ "test": all_kept, "production": paid_dropped }),
        ),
        (
            "limited-offer",
            "dev",
            offer,
            CatalogueSettings::default(),
            json!({ "test": all_kept, "production": paid_dropped }),
        ),
        (
            "staging-with-the-fake-provider",
            "staging",
            three,
            staging(),
            json!({ "test": free_only, "production": free_only }),
        ),
        (
            "repository-nothing-offered",
            "dev",
            repository,
            CatalogueSettings::default(),
            json!({ "test": empty, "production": empty }),
        ),
    ]
}

async fn vectors() -> Value {
    let mut list = Vec::new();
    for (name, env, rows, settings, expect) in cases().await {
        let body = catalogue::public(&rows, &settings, vectors_now());
        let bytes = serde_json::to_vec(&body).unwrap();
        list.push(json!({
            "name": name,
            "env": env,
            "bytes": bytes.len(),
            "body": serde_json::from_slice::<Value>(&bytes).unwrap(),
            "expect": expect,
        }));
    }
    json!({
        "note": "Answers of GET /v1/plans this server builds (tests/plans.rs, server_catalogs), with what the \
                 extension's parseCatalog must make of each: `test` with the currencies of a test or development \
                 build (Intl plus XTS), `production` with Intl's alone. Placeholder texts and the fake provider's \
                 prices in XTS only.",
        "payHosts": ["web.example.invalid"],
        "now": vectors_now().unix_timestamp(),
        "cases": list,
    })
}

#[tokio::test]
async fn server_catalogs() {
    let built = vectors().await;
    let path = support::repo(VECTORS);
    if std::env::var("DEVTAB_WRITE_VECTORS").is_ok_and(|v| v == "1") {
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&built).unwrap()),
        )
        .unwrap();
        return;
    }
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        stored, built,
        "{VECTORS} is not what the server builds: DEVTAB_WRITE_VECTORS=1 cargo test -p devtab-api \
         --test plans server_catalogs"
    );
}

#[tokio::test]
async fn every_answer_follows_the_schema_and_its_size_limit() {
    for case in vectors().await["cases"].as_array().unwrap() {
        let found = problems(&support::schema::load(SCHEMA), &case["body"]);
        assert!(found.is_empty(), "{}: {found:?}", case["name"]);
        assert!(case["bytes"].as_u64().unwrap() <= 32 * 1024);
    }
    // The longest catalogue the file rules allow still fits: six plans with
    // every text at its limit, in characters of four bytes each.
    let long = |n: usize| "\u{1f600}".repeat(n);
    let mut rows = rows_of(&fixture(), "2026-11-24.1").await;
    rows.plans = (0..6)
        .map(|i| PlanRow {
            id: format!("plan-{i}-{}", "x".repeat(25)),
            ..rows.plans[1].clone()
        })
        .collect();
    rows.texts = rows
        .plans
        .iter()
        .map(|p| PlanTextRow {
            plan_id: p.id.clone(),
            locale: "en".to_owned(),
            name: long(32),
            summary: Some(long(140)),
            includes: vec![long(100); 8],
            price_note: Some(long(60)),
            badge: Some(long(24)),
        })
        .collect();
    rows.prices = rows
        .plans
        .iter()
        .map(|p| PriceRow {
            plan_id: p.id.clone(),
            ..rows.prices[0].clone()
        })
        .collect();
    let body = catalogue::public(&rows, &CatalogueSettings::default(), vectors_now());
    assert_eq!(body.plans.len(), 6);
    let bytes = serde_json::to_vec(&body).unwrap().len();
    assert!(bytes <= 32 * 1024, "{bytes} bytes");
}

// ------------------------------------------------------------------------------------------------
// The sync, in a transaction that is rolled back

/// A transaction acting as the API role, in which the catalogue is empty
/// but for the plans' rows, none of them available: it holds only what the
/// test syncs, whatever the local database had.
async fn begin(conn: &mut PgConnection) -> sqlx::Transaction<'_, sqlx::Postgres> {
    let mut tx = conn.begin().await.unwrap();
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    for statement in [
        "update app.plans set available = false",
        "delete from app.plan_texts",
        "delete from app.plan_prices",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx
}

async fn sync_with(
    tx: &mut PgConnection,
    path: &Path,
    providers: &[&dyn PaymentProvider],
    confirm: bool,
) -> anyhow::Result<String> {
    let plans_file = file::read(path)?;
    let mut out = Vec::new();
    let mut ctx = Context {
        actor: "tester".to_owned(),
        now: now(),
        out: &mut out,
    };
    sync::sync(
        &plans_file,
        providers,
        &CatalogueSettings::default(),
        tx,
        &mut ctx,
        &mut || confirm,
    )
    .await?;
    Ok(String::from_utf8(out).unwrap())
}

/// The stored catalogue, read as the endpoints read it (the fake provider's
/// prices).
async fn stored(tx: &mut PgConnection) -> CatalogueRows {
    CatalogueRows {
        version: store::catalog_version(&mut *tx).await.unwrap(),
        plans: store::all(&mut *tx).await.unwrap(),
        texts: store::texts(&mut *tx).await.unwrap(),
        prices: store::active_prices(&mut *tx, "fake").await.unwrap(),
    }
}

/// A temporary plans file.
struct TempFile(PathBuf);

impl TempFile {
    fn new(text: &str) -> Self {
        let path = std::env::temp_dir().join(format!("devtab-plans-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, text).unwrap();
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[tokio::test]
async fn sync_writes_plans_texts_and_prices_and_the_answer_is_the_vector_s() {
    let Some(mut conn) = support::connect().await else {
        return;
    };
    let _turn = CATALOGUE.lock().await;
    let mut tx = begin(&mut conn).await;

    let declined = sync_with(&mut tx, &fixture(), &[], false).await.unwrap();
    // The plans' rows may be there already (`~`); their texts and prices are not.
    assert!(
        declined.contains("+ plan pro-monthly") || declined.contains("~ plan pro-monthly"),
        "{declined}"
    );
    for line in [
        "+ texts pro-annual/en",
        "+ price pro-monthly fake pri_TEST_0001  XTS 1111",
        "nothing changed",
    ] {
        assert!(declined.contains(line), "{line}: {declined}");
    }
    let before = stored(&mut tx).await.version;

    let applied = sync_with(&mut tx, &fixture(), &[], true).await.unwrap();
    assert!(applied.contains("written:"), "{applied}");
    let rows = stored(&mut tx).await;
    let version = rows.version.clone().unwrap();
    assert_eq!(version, sync::next_version(before.as_deref(), now()));
    assert!(devtab_api::plans::is_catalog_id(&version));

    // What /v1/plans answers from the stored rows is the vector's answer.
    let answer = serde_json::to_value(catalogue::public(
        &rows,
        &CatalogueSettings::default(),
        vectors_now(),
    ))
    .unwrap();
    let vectors: Value =
        serde_json::from_str(&std::fs::read_to_string(support::repo(VECTORS)).unwrap()).unwrap();
    let mut expected = vectors["cases"][0]["body"].clone();
    expected["catalog"] = Value::from(version.clone());
    assert_eq!(answer, expected);

    let again = sync_with(&mut tx, &fixture(), &[], true).await.unwrap();
    assert!(again.contains("the catalogue is up to date"), "{again}");
    assert_eq!(
        stored(&mut tx).await.version.as_deref(),
        Some(version.as_str())
    );

    // A changed text, a new language, a price that moves to another id.
    let changed = std::fs::read_to_string(fixture())
        .unwrap()
        .replace("name = \"Monthly\"", "name = \"Every month\"")
        .replace("pri_TEST_0002", "pri_TEST_0003")
        .replace("amount_minor = 11110", "amount_minor = 11111")
        + "[plan.texts.ru]\nname = \"Ежегодно\"\n";
    let changed = TempFile::new(&changed);
    let output = sync_with(&mut tx, &changed.0, &[], true).await.unwrap();
    for line in [
        "~ texts pro-monthly/en",
        "+ texts pro-annual/ru",
        "- price pro-annual fake pri_TEST_0002  XTS 11110 (deactivated)",
        "+ price pro-annual fake pri_TEST_0003  XTS 11111",
    ] {
        assert!(output.contains(line), "{line}: {output}");
    }
    let prices = store::prices(&mut *tx).await.unwrap();
    let state = |id: &str| {
        prices
            .iter()
            .find(|p| p.provider_price_id == id)
            .unwrap()
            .active
    };
    assert!(
        !state("pri_TEST_0002"),
        "a price that left the file is kept, inactive"
    );
    assert!(state("pri_TEST_0003"));
    let site = catalogue::site(
        &stored(&mut tx).await,
        &CatalogueSettings::default(),
        "ru",
        now(),
    );
    assert_eq!(site.plans[2].plan.name, "Ежегодно");
    assert_eq!(site.plans[1].plan.name, "Every month");
    assert_eq!(site.plans[1].locale, "en", "no Russian texts: English");

    // Back to the fixture: the Russian texts go, the old price comes back.
    let output = sync_with(&mut tx, &fixture(), &[], true).await.unwrap();
    assert!(output.contains("- texts pro-annual/ru"), "{output}");
    assert!(
        output.contains("~ price pro-annual fake pri_TEST_0002  XTS 11110 (inactive) -> XTS 11110"),
        "{output}"
    );

    let audit: Vec<Value> = sqlx::query_scalar(
        "select details from app.admin_audit where actor = 'tester' and action = 'plans.sync' order by id",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audit.len(), 3, "one record per applied sync");
    assert_eq!(audit[0]["catalog"], Value::from(version));
    assert_eq!(
        audit[1]["prices"],
        json!(["fake:pri_TEST_0003", "-fake:pri_TEST_0002"])
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn sync_refuses_what_the_extension_would_skip() {
    let Some(mut conn) = support::connect().await else {
        return;
    };
    let _turn = CATALOGUE.lock().await;
    let mut tx = begin(&mut conn).await;
    let base = std::fs::read_to_string(fixture()).unwrap();
    let refused = [
        // A text longer than its limit in code points (25 of 24).
        base.replace(
            "\"Save with a year\"",
            &format!("\"{}\"", "\u{1f600}".repeat(25)),
        ),
        // XTS at another provider.
        base.replacen("provider = \"fake\"", "provider = \"other\"", 1),
        // A control character.
        base.replace(
            "\"Pro, billed once a year.\"",
            "\"Pro,\\u0007 billed once a year.\"",
        ),
    ];
    for text in refused {
        let temp = TempFile::new(&text);
        assert!(sync_with(&mut tx, &temp.0, &[], true).await.is_err());
    }

    // A seventh available plan, counting the plans that are not in the file.
    for i in 0..4 {
        store::upsert(
            &mut *tx,
            &PlanRow {
                id: format!("test-cat-extra-{i}"),
                kind: "free".to_owned(),
                interval: None,
                seats: None,
                sort: 100,
                available: true,
                until: None,
                min_members: None,
            },
        )
        .await
        .unwrap();
    }
    let err = sync_with(&mut tx, &fixture(), &[], true)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("7 plans would be available"), "{err}");
    let audit: i64 = sqlx::query_scalar(
        "select count(*) from app.admin_audit where actor = 'tester' and action = 'plans.sync'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audit, 0, "nothing was written");
    tx.rollback().await.unwrap();
}

/// A provider that answers like a remote one: later, with its own price ids.
#[derive(Debug)]
struct Remote;

impl PaymentProvider for Remote {
    fn id(&self) -> &'static str {
        "remote"
    }

    fn price<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PriceSnapshot> {
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            if id == "price_remote_monthly" {
                Ok(PriceSnapshot {
                    currency: "EUR".to_owned(),
                    amount_minor: 1111,
                })
            } else {
                Err(ProviderError::NotFound(format!("price {id}")))
            }
        })
    }
}

#[tokio::test]
async fn a_real_provider_s_amount_is_read_through_its_adapter() {
    let Some(mut conn) = support::connect().await else {
        return;
    };
    let _turn = CATALOGUE.lock().await;
    let mut tx = begin(&mut conn).await;
    let text = std::fs::read_to_string(fixture()).unwrap().replace(
        "[[plan.prices]]\nprovider = \"fake\"\nprovider_price_id = \"pri_TEST_0001\"\ncurrency = \"XTS\"\namount_minor = 1111\n",
        "[[plan.prices]]\nprovider = \"remote\"\nprovider_price_id = \"price_remote_monthly\"\n",
    );
    let temp = TempFile::new(&text);
    assert!(
        sync_with(&mut tx, &temp.0, &[], true)
            .await
            .unwrap_err()
            .to_string()
            .contains("no adapter")
    );
    sync_with(&mut tx, &temp.0, &[&Remote], true).await.unwrap();
    let row = store::active_prices(&mut *tx, "remote").await.unwrap();
    assert_eq!(
        row.iter()
            .map(|p| (p.plan_id.as_str(), p.currency.as_str(), p.amount_minor))
            .collect::<Vec<_>>(),
        [("pro-monthly", "EUR", 1111)]
    );
    // Served only where that provider is the current one.
    let mut rows = stored(&mut tx).await;
    rows.prices = store::active_prices(&mut *tx, "remote").await.unwrap();
    let remote = CatalogueSettings {
        provider: Some("remote".to_owned()),
        test_currencies: false,
        ..CatalogueSettings::default()
    };
    let answer = catalogue::public(&rows, &remote, now());
    let ids: Vec<&str> = answer.plans.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["free", "pro-monthly"]);
    tx.rollback().await.unwrap();
}

// ------------------------------------------------------------------------------------------------
// The endpoints

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    bytes: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap()
    }
}

async fn send(app: &Router, request: Request<Body>) -> Answer {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Answer {
        status,
        headers,
        bytes,
    }
}

fn get(path: &str) -> axum::http::request::Builder {
    Request::get(path).header(ORIGIN, ORIGIN_OF_A_PAGE)
}

fn config() -> Config {
    Config {
        rate_limits: RateLimits {
            plans_per_ip: NonZeroU32::new(1_000).unwrap(),
            ..RateLimits::default()
        },
        ..Config::default()
    }
}

fn check_error(answer: &Answer, code: &str) {
    let body = answer.json();
    assert_eq!(body["error"], code);
    assert!(body["message"].is_string());
    let errors = support::schema::load("errors.json");
    assert!(
        errors["endpoints"]["plans"]
            .as_array()
            .unwrap()
            .contains(&Value::from(code)),
        "plans may not answer {code}"
    );
}

#[tokio::test]
async fn the_guards_answer_with_cors_for_any_origin() {
    let config = config();
    let app = public_app(&config, support::offline_state(&config));

    // A preflight, which a simple GET never needs.
    let preflight = send(
        &app,
        Request::builder()
            .method(Method::OPTIONS)
            .uri("/v1/plans")
            .header(ORIGIN, ORIGIN_OF_A_PAGE)
            .header("access-control-request-method", "GET")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(preflight.status, StatusCode::NO_CONTENT);
    assert!(preflight.bytes.is_empty());
    assert_eq!(preflight.headers[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    assert_eq!(preflight.headers[ACCESS_CONTROL_ALLOW_METHODS], "GET");
    assert_eq!(preflight.headers[ACCESS_CONTROL_MAX_AGE], "86400");
    assert!(
        preflight
            .headers
            .get(ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .is_none()
    );

    // No database: 503 without Retry-After, still readable by any origin.
    let down = send(&app, get("/v1/plans").body(Body::empty()).unwrap()).await;
    assert_eq!(down.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(down.headers.get(RETRY_AFTER).is_none());
    assert_eq!(down.headers[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    check_error(&down, "unavailable");

    let post = send(
        &app,
        Request::post("/v1/plans").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(post.status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(post.headers[ALLOW].to_str().unwrap().contains("GET"));
    assert_eq!(post.headers[ACCESS_CONTROL_ALLOW_ORIGIN], "*");

    // The internal route is not on the public listener, nor /v1/plans on the internal one.
    let internal = send(&app, get("/internal/plans").body(Body::empty()).unwrap()).await;
    assert_eq!(internal.status, StatusCode::NOT_FOUND);
    let internal_app = internal_app(&config, support::offline_state(&config));
    let public = send(&internal_app, get("/v1/plans").body(Body::empty()).unwrap()).await;
    assert_eq!(public.status, StatusCode::NOT_FOUND);
    let unknown = send(
        &internal_app,
        get("/internal/plans?locale=de")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    assert_eq!(unknown.json()["error"], "bad-request");
}

#[tokio::test]
async fn host_maintenance_and_the_rate_limit_hold_for_plans_too() {
    let config = Config {
        licence_host: Some("licence.example.invalid".to_owned()),
        maintenance_until: Some(now() + time::Duration::minutes(10)),
        ..config()
    };
    let app = public_app(&config, support::offline_state(&config));
    let elsewhere = send(
        &app,
        get("/v1/plans")
            .header(HOST, "web.example.invalid")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(elsewhere.status, StatusCode::NOT_FOUND);
    let closed = send(
        &app,
        get("/v1/plans")
            .header(HOST, "licence.example.invalid")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(closed.status, StatusCode::SERVICE_UNAVAILABLE);
    let seconds: u64 = closed.headers[RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((590..=600).contains(&seconds), "{seconds}");
    assert_eq!(closed.headers[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    check_error(&closed, "unavailable");

    let limited = Config {
        rate_limits: RateLimits {
            plans_per_ip: NonZeroU32::new(2).unwrap(),
            ..RateLimits::default()
        },
        ..Config::default()
    };
    let app = public_app(&limited, support::offline_state(&limited));
    for _ in 0..2 {
        let answer = send(&app, get("/v1/plans").body(Body::empty()).unwrap()).await;
        assert_eq!(
            answer.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "no database"
        );
    }
    let third = send(&app, get("/v1/plans").body(Body::empty()).unwrap()).await;
    assert_eq!(third.status, StatusCode::TOO_MANY_REQUESTS);
    let seconds: u64 = third.headers[RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&seconds), "two a minute: {seconds}");
    assert_eq!(third.headers[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    check_error(&third, "rate-limited");
}

const TEST_PLANS: &str = "test-cat-%";

async fn remove_test_plans(owner: &mut PgConnection) {
    sqlx::query("delete from app.plan_prices where plan_id like $1")
        .bind(TEST_PLANS)
        .execute(&mut *owner)
        .await
        .unwrap();
    sqlx::query("delete from app.plans where id like $1")
        .bind(TEST_PLANS)
        .execute(&mut *owner)
        .await
        .unwrap();
}

/// Committed plans sorted before any other: free and monthly available, a
/// yearly one that is not, and one whose offer has ended.
async fn insert_test_plans(owner: &mut PgConnection, clock_now: OffsetDateTime) {
    let plan =
        |id: &str, kind: &str, interval: Option<&str>, sort: i16, available: bool, until| PlanRow {
            id: id.to_owned(),
            kind: kind.to_owned(),
            interval: interval.map(str::to_owned),
            seats: (kind != "free").then_some(3),
            sort,
            available,
            until,
            min_members: None,
        };
    let plans = [
        plan("test-cat-free", "free", None, -32_000, true, None),
        plan(
            "test-cat-month",
            "subscription",
            Some("month"),
            -31_999,
            true,
            None,
        ),
        plan(
            "test-cat-year",
            "subscription",
            Some("year"),
            -31_998,
            false,
            None,
        ),
        plan(
            "test-cat-ended",
            "subscription",
            Some("month"),
            -31_997,
            true,
            Some(clock_now - time::Duration::seconds(1)),
        ),
    ];
    for row in &plans {
        store::upsert(&mut *owner, row).await.unwrap();
        store::upsert_text(
            &mut *owner,
            &PlanTextRow {
                plan_id: row.id.clone(),
                locale: "en".to_owned(),
                name: format!("Test {}", &row.id[9..]),
                summary: None,
                includes: vec!["A line".to_owned()],
                price_note: None,
                badge: None,
            },
        )
        .await
        .unwrap();
        if row.kind != "free" {
            store::insert_price(
                &mut *owner,
                &NewPrice {
                    plan_id: row.id.clone(),
                    provider: "fake".to_owned(),
                    provider_price_id: format!("pri_TEST_{}", row.id),
                    currency: "XTS".to_owned(),
                    amount_minor: 1111,
                },
            )
            .await
            .unwrap();
        }
    }
    store::upsert_text(
        &mut *owner,
        &PlanTextRow {
            plan_id: "test-cat-month".to_owned(),
            locale: "ru".to_owned(),
            name: "Тест месяц".to_owned(),
            summary: None,
            includes: vec![],
            price_note: None,
            badge: None,
        },
    )
    .await
    .unwrap();
}

fn ours(plans: &Value) -> Vec<String> {
    plans
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .filter(|id| id.starts_with("test-cat-"))
        .collect()
}

#[tokio::test]
async fn the_endpoints_answer_from_the_stored_catalogue() {
    let Some(url) = support::database_url() else {
        return;
    };
    let _turn = CATALOGUE.lock().await;
    let mut owner = PgConnection::connect(&url).await.unwrap();
    let clock_now = now();
    remove_test_plans(&mut owner).await;
    insert_test_plans(&mut owner, clock_now).await;
    let clock = Arc::new(ManualClock::new(clock_now));

    let config = config();
    let state = support::api_state(&url, clock.clone(), &config).await;
    let public = public_app(&config, state.clone());
    let internal = internal_app(&config, state);

    let first = send(&public, get("/v1/plans").body(Body::empty()).unwrap()).await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.headers[CONTENT_TYPE], "application/json");
    assert_eq!(first.headers[CACHE_CONTROL], "no-cache");
    assert_eq!(first.headers[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    assert!(
        first
            .headers
            .get(ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .is_none()
    );
    let tag = first.headers[ETAG].to_str().unwrap().to_owned();
    assert!(first.bytes.len() <= 32 * 1024);
    let body = first.json();
    let found = problems(&support::schema::load(SCHEMA), &body);
    assert!(found.is_empty(), "{found:?}");
    assert_eq!(ours(&body["plans"]), ["test-cat-free", "test-cat-month"]);
    assert_eq!(
        body["plans"][1]["price"],
        json!({ "amount": 1111, "currency": "XTS" })
    );
    assert_eq!(
        body["plans"][1]["checkoutUrl"],
        "https://web.example.invalid/pro/checkout/?plan=test-cat-month"
    );
    assert_eq!(body["manageUrl"], "https://web.example.invalid/pro/manage/");
    assert_eq!(body["maxAge"], 86_400);

    // The same tag again: 304 without a body, with the tag and CORS.
    let again = send(
        &public,
        get("/v1/plans")
            .header(IF_NONE_MATCH, &tag)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(again.status, StatusCode::NOT_MODIFIED);
    assert!(again.bytes.is_empty());
    assert_eq!(again.headers[ETAG], tag.as_str());
    assert_eq!(again.headers[CACHE_CONTROL], "no-cache");
    assert_eq!(again.headers[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    let other = send(
        &public,
        get("/v1/plans")
            .header(IF_NONE_MATCH, "\"other\"")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(other.status, StatusCode::OK);

    // The site's pages: every plan whose offer has not ended, in Russian where there is Russian.
    let site = send(
        &internal,
        get("/internal/plans?locale=ru")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(site.status, StatusCode::OK);
    let site = site.json();
    assert_eq!(site["locale"], "ru");
    assert_eq!(
        ours(&site["plans"]),
        ["test-cat-free", "test-cat-month", "test-cat-year"]
    );
    let month = &site["plans"][1];
    assert_eq!(
        (&month["name"], &month["locale"], &month["available"]),
        (&json!("Тест месяц"), &json!("ru"), &json!(true))
    );
    assert_eq!(site["plans"][2]["available"], false);
    assert_eq!(site["plans"][2]["price"]["amount"], 1111, "shown, not sold");
    let english = send(
        &internal,
        Request::get("/internal/plans").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(english.json()["plans"][1]["name"], "Test month");

    // Staging serves no test currency: the paid test plans stay out.
    let staging_config = Config {
        env: devtab_api::config::Env::Staging,
        ..config.clone()
    };
    let staging_state = support::api_state(&url, clock.clone(), &staging_config).await;
    assert!(!staging_state.catalogue.test_currencies);
    let staging_app = public_app(&staging_config, staging_state);
    let answer = send(&staging_app, get("/v1/plans").body(Body::empty()).unwrap()).await;
    assert_eq!(ours(&answer.json()["plans"]), ["test-cat-free"]);

    // The ended offer would come back with the clock turned back.
    clock.set(clock_now - time::Duration::hours(1));
    let earlier = send(&public, get("/v1/plans").body(Body::empty()).unwrap()).await;
    assert_eq!(
        ours(&earlier.json()["plans"]),
        ["test-cat-free", "test-cat-month", "test-cat-ended"]
    );

    remove_test_plans(&mut owner).await;
}
