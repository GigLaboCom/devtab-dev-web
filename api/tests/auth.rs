//! Signed-in people: Supabase access tokens checked against a JWKS, and the
//! `CurrentUser` of `GET /internal/me`, without any real sign-in provider.
//!
//! Tokens are signed with a P-256 key made in memory for this run, and the
//! verifier gets its public half as a fixed set or from a JWKS served on a
//! loopback port; no key is stored. The tests that need people run against
//! `DEVTAB_TEST_DATABASE_URL` (skipped without it): they insert an
//! `auth.users` row, as Supabase Auth would, which the trigger gives an
//! `app.users` row, and delete it at the end.
#![allow(clippy::unwrap_used)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use devtab_api::auth::{TokenError, Verifier};
use devtab_api::clock::{Clock as _, ManualClock, SharedClock};
use devtab_api::licence::signer;
use devtab_api::{AppState, Config, internal_app};
use http_body_util::BodyExt as _;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt as _;
use uuid::Uuid;

const ISSUER: &str = "http://127.0.0.1:54332/auth/v1";

/// A P-256 key pair for one test: the key to sign with and its JWKS entry.
struct Keys {
    kid: String,
    encoding: EncodingKey,
    jwk: Value,
}

fn keys() -> Keys {
    let (pem, entry) = signer::generate(None, true, OffsetDateTime::now_utc()).unwrap();
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let der = STANDARD.decode(body).unwrap();
    Keys {
        jwk: json!({
            "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig",
            "kid": entry.kid, "x": entry.jwk.x, "y": entry.jwk.y,
        }),
        kid: entry.kid,
        encoding: EncodingKey::from_ec_der(&der),
    }
}

/// A set with `keys` and a symmetric key, which the verifier must leave out.
fn jwks(keys: &[&Keys]) -> Value {
    let mut list: Vec<Value> = keys.iter().map(|k| k.jwk.clone()).collect();
    list.push(json!({ "kty": "oct", "kid": "shared", "k": "c2VjcmV0" }));
    json!({ "keys": list })
}

fn clock() -> ManualClock {
    ManualClock::new(OffsetDateTime::now_utc())
}

fn claims(sub: impl Into<Value>, clock: &ManualClock) -> Value {
    json!({
        "sub": sub.into(),
        "aud": "authenticated",
        "iss": ISSUER,
        "exp": (clock.now() + Duration::hours(1)).unix_timestamp(),
        "role": "authenticated",
        "email": "person@example.invalid",
    })
}

fn sign_as(keys: &Keys, kid: &str, claims: &Value) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_owned());
    jsonwebtoken::encode(&header, claims, &keys.encoding).unwrap()
}

fn sign(keys: &Keys, claims: &Value) -> String {
    sign_as(keys, &keys.kid, claims)
}

fn fixed(keys: &Keys, clock: &ManualClock) -> Verifier {
    Verifier::fixed(&jwks(&[keys]), ISSUER.to_owned(), shared(clock))
}

fn shared(clock: &ManualClock) -> SharedClock {
    Arc::new(clock.clone())
}

#[tokio::test]
async fn a_good_token_gives_its_subject() {
    let (keys, clock) = (keys(), clock());
    let sub = Uuid::new_v4();
    let got = fixed(&keys, &clock)
        .verify(&sign(&keys, &claims(sub.to_string(), &clock)))
        .await
        .unwrap();
    assert_eq!(got.sub, sub);
}

#[tokio::test]
async fn wrong_claims_are_refused() {
    let (keys, clock) = (keys(), clock());
    let verifier = fixed(&keys, &clock);
    let sub = Uuid::new_v4().to_string();
    let with = |key: &str, value: Value| {
        let mut c = claims(sub.clone(), &clock);
        c[key] = value;
        sign(&keys, &c)
    };
    let now = clock.now().unix_timestamp();
    for (what, token) in [
        (
            "issuer",
            with("iss", json!("https://other.example.invalid/auth/v1")),
        ),
        ("audience", with("aud", json!("anon"))),
        ("expired past the leeway", with("exp", json!(now - 31))),
        ("subject not a UUID", with("sub", json!("not-a-uuid"))),
        ("anonymous session", with("is_anonymous", json!(true))),
    ] {
        assert!(verifier.verify(&token).await.is_err(), "{what}");
    }
    assert!(matches!(
        verifier.verify(&with("exp", json!(now - 31))).await,
        Err(TokenError::Expired)
    ));
    assert!(matches!(
        verifier.verify(&with("is_anonymous", json!(true))).await,
        Err(TokenError::Anonymous)
    ));
    // Within the 30 seconds of leeway, by the API's clock.
    assert!(verifier.verify(&with("exp", json!(now - 29))).await.is_ok());
    // An hour's token, two hours on by the API's clock: expired; a new one passes.
    let earlier = sign(&keys, &claims(sub.clone(), &clock));
    clock.advance(Duration::hours(2));
    assert!(matches!(
        verifier.verify(&earlier).await,
        Err(TokenError::Expired)
    ));
    let later = sign(&keys, &claims(sub.clone(), &clock));
    assert!(verifier.verify(&later).await.is_ok());
    let mut no_exp = claims(sub, &clock);
    no_exp.as_object_mut().unwrap().remove("exp");
    assert!(verifier.verify(&sign(&keys, &no_exp)).await.is_err());
}

#[tokio::test]
async fn other_keys_and_algorithms_are_refused() {
    let (keys, clock) = (keys(), clock());
    let verifier = fixed(&keys, &clock);
    let good = claims(Uuid::new_v4().to_string(), &clock);

    // Another key under the same kid.
    let other = self::keys();
    assert!(matches!(
        verifier.verify(&sign_as(&other, &keys.kid, &good)).await,
        Err(TokenError::Invalid(_))
    ));
    // A kid the set does not have.
    assert!(matches!(
        verifier.verify(&sign(&other, &good)).await,
        Err(TokenError::UnknownKey)
    ));
    // HS256 with the set's symmetric key.
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("shared".to_owned());
    let token = jsonwebtoken::encode(&header, &good, &EncodingKey::from_secret(b"secret")).unwrap();
    assert!(matches!(
        verifier.verify(&token).await,
        Err(TokenError::Algorithm)
    ));
    // RS256 named for the ES256 key: refused before any signature is checked.
    let part = |v: Value| URL_SAFE_NO_PAD.encode(v.to_string());
    let token = format!(
        "{}.{}.{}",
        part(json!({ "alg": "RS256", "typ": "JWT", "kid": keys.kid })),
        part(good.clone()),
        URL_SAFE_NO_PAD.encode([0_u8; 256])
    );
    assert!(matches!(
        verifier.verify(&token).await,
        Err(TokenError::Algorithm)
    ));
    // `none`.
    let token = format!("{}.{}.", part(json!({ "alg": "none" })), part(good.clone()));
    assert!(verifier.verify(&token).await.is_err());
    // A changed payload under a good signature.
    let token = sign(&keys, &good);
    let mut parts: Vec<String> = token.split('.').map(str::to_owned).collect();
    parts[1] = part(claims(Uuid::new_v4().to_string(), &clock));
    assert!(matches!(
        verifier.verify(&parts.join(".")).await,
        Err(TokenError::Invalid(_))
    ));
    assert!(matches!(
        verifier.verify("not a token").await,
        Err(TokenError::Malformed)
    ));
}

type Served = Arc<std::sync::Mutex<Option<Value>>>;

/// A JWKS on a loopback port that counts its fetches; it serves `set`, or 503
/// while `set` is `None`.
async fn jwks_server(set: Served) -> (String, Arc<AtomicUsize>) {
    use axum::response::IntoResponse as _;
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = axum::Router::new().route(
        "/auth/v1/.well-known/jwks.json",
        axum::routing::get(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let body = set.lock().unwrap().clone();
            async move {
                match body {
                    Some(body) => axum::Json(body).into_response(),
                    None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/auth/v1/.well-known/jwks.json"), hits)
}

#[tokio::test]
async fn the_set_is_kept_five_minutes_and_an_unknown_kid_fetches_it_at_most_every_30_seconds() {
    let (keys, clock) = (keys(), clock());
    let set: Served = Arc::new(std::sync::Mutex::new(Some(jwks(&[&keys]))));
    let (url, hits) = jwks_server(set.clone()).await;
    let verifier = Verifier::remote(url, ISSUER.to_owned(), shared(&clock)).unwrap();
    let token = |k: &Keys| sign(k, &claims(Uuid::new_v4().to_string(), &clock));

    verifier.verify(&token(&keys)).await.unwrap();
    verifier.verify(&token(&keys)).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the set is kept");

    // The project rotates to a new key; a token signed with it arrives at once.
    let rotated = self::keys();
    *set.lock().unwrap() = Some(jwks(&[&keys, &rotated]));
    assert!(matches!(
        verifier.verify(&token(&rotated)).await,
        Err(TokenError::UnknownKey)
    ));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "no fetch within 30 s of the last"
    );
    clock.advance(Duration::seconds(31));
    verifier.verify(&token(&rotated)).await.unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "the unknown kid fetched the set"
    );
    // A made-up kid right after: no fetch.
    let stranger = self::keys();
    assert!(verifier.verify(&token(&stranger)).await.is_err());
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    clock.advance(Duration::minutes(5));
    verifier.verify(&token(&keys)).await.unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        3,
        "after five minutes the set is fetched again"
    );
}

#[tokio::test]
async fn an_unreachable_set_is_unavailable_and_kept_keys_still_work() {
    let (keys, clock) = (keys(), clock());
    let verifier = Verifier::remote(
        "http://127.0.0.1:1/auth/v1/.well-known/jwks.json".to_owned(),
        ISSUER.to_owned(),
        shared(&clock),
    )
    .unwrap();
    let token = sign(&keys, &claims(Uuid::new_v4().to_string(), &clock));
    assert!(matches!(
        verifier.verify(&token).await,
        Err(TokenError::KeysUnavailable(_))
    ));

    let set: Served = Arc::new(std::sync::Mutex::new(Some(jwks(&[&keys]))));
    let (url, hits) = jwks_server(set.clone()).await;
    let verifier = Verifier::remote(url, ISSUER.to_owned(), shared(&clock)).unwrap();
    verifier.verify(&token).await.unwrap();
    // The JWKS fails: past five minutes the fetch fails, and the kept key still works.
    *set.lock().unwrap() = None;
    clock.advance(Duration::minutes(6));
    let token = sign(&keys, &claims(Uuid::new_v4().to_string(), &clock));
    verifier.verify(&token).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 2, "it tried, and kept the key");
    // A key it never had cannot be checked: unavailable, not unknown.
    clock.advance(Duration::minutes(1));
    let other = self::keys();
    assert!(matches!(
        verifier
            .verify(&sign(&other, &claims(Uuid::new_v4().to_string(), &clock)))
            .await,
        Err(TokenError::KeysUnavailable(_))
    ));
}

// ---- GET /internal/me ----

async fn get_me(
    state: AppState,
    token: Option<&str>,
    extra: &[(&str, &str)],
    path: &str,
) -> (StatusCode, Value, HeaderMap) {
    let mut request = Request::get(path);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let response = internal_app(&Config::default(), state)
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        headers,
    )
}

fn offline(clock: &ManualClock) -> AppState {
    let mut state = support::offline_state(&Config::default());
    state.clock = shared(clock);
    state
}

#[tokio::test]
async fn me_needs_a_token_that_passes() {
    let (keys, clock) = (keys(), clock());
    let state = offline(&clock).with_auth(fixed(&keys, &clock));

    let (status, body, headers) = get_me(state.clone(), None, &[], "/internal/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
    assert_eq!(headers["www-authenticate"], "Bearer");

    for token in [
        "not.a.token",
        &sign(&self::keys(), &claims(Uuid::new_v4().to_string(), &clock)),
    ] {
        let (status, body, headers) = get_me(state.clone(), Some(token), &[], "/internal/me").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(headers["www-authenticate"], "Bearer");
    }
    let (status, _, _) = get_me(
        state,
        None,
        &[("authorization", "Basic dXNlcjpwYXNz")],
        "/internal/me",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_is_unavailable_without_sign_in_or_without_the_keys() {
    let (keys, clock) = (keys(), clock());
    let token = sign(&keys, &claims(Uuid::new_v4().to_string(), &clock));
    let (status, body, _) = get_me(offline(&clock), Some(&token), &[], "/internal/me").await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("unavailable"))
    );

    let unreachable = Verifier::remote(
        "http://127.0.0.1:1/auth/v1/.well-known/jwks.json".to_owned(),
        ISSUER.to_owned(),
        shared(&clock),
    )
    .unwrap();
    let state = offline(&clock).with_auth(unreachable);
    let (status, _, headers) = get_me(state, Some(&token), &[], "/internal/me").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "not 401");
    assert!(headers.get("www-authenticate").is_none());
}

#[tokio::test]
async fn me_is_the_tokens_person_never_one_named_elsewhere() {
    let Some(url) = support::database_url() else {
        return;
    };
    let mut owner = support::connect().await.unwrap();
    let (keys, clock) = (keys(), clock());
    let person = support::insert_auth_user(&mut owner, "signed-in@example.invalid").await;
    let someone = support::insert_auth_user(&mut owner, "someone@example.invalid").await;
    let state = support::api_state(&url, shared(&clock), &Config::default())
        .await
        .with_auth(fixed(&keys, &clock));
    let token = sign(&keys, &claims(person.to_string(), &clock));

    let path = format!("/internal/me?user={someone}");
    let header = someone.to_string();
    let (status, body, headers) =
        get_me(state.clone(), Some(&token), &[("x-user", &header)], &path).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(
        body,
        json!({ "user": person, "email": "signed-in@example.invalid" })
    );

    // A token that passes for someone the API has no row for.
    let stranger = sign(&keys, &claims(Uuid::new_v4().to_string(), &clock));
    let (status, _, _) = get_me(state, Some(&stranger), &[], "/internal/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    for id in [person, someone] {
        sqlx::query("delete from auth.users where id = $1")
            .bind(id)
            .execute(&mut owner)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_blocked_account_is_forbidden() {
    let Some(url) = support::database_url() else {
        return;
    };
    let mut owner = support::connect().await.unwrap();
    let (keys, clock) = (keys(), clock());
    let person = support::insert_auth_user(&mut owner, "blocked@example.invalid").await;
    sqlx::query("update app.users set status = 'blocked' where id = $1")
        .bind(person)
        .execute(&mut owner)
        .await
        .unwrap();
    let state = support::api_state(&url, shared(&clock), &Config::default())
        .await
        .with_auth(fixed(&keys, &clock));
    let token = sign(&keys, &claims(person.to_string(), &clock));
    let (status, body, _) = get_me(state.clone(), Some(&token), &[], "/internal/me").await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("forbidden"))
    );

    // Unblocked, the cached row is used for a minute, then read again.
    sqlx::query("update app.users set status = 'active' where id = $1")
        .bind(person)
        .execute(&mut owner)
        .await
        .unwrap();
    let (status, _, _) = get_me(state.clone(), Some(&token), &[], "/internal/me").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    clock.advance(Duration::seconds(61));
    let token = sign(&keys, &claims(person.to_string(), &clock));
    let (status, _, _) = get_me(state, Some(&token), &[], "/internal/me").await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("delete from auth.users where id = $1")
        .bind(person)
        .execute(&mut owner)
        .await
        .unwrap();
}
