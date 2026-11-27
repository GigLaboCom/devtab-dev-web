//! The extension's licence endpoints through the public router (`oneshot`,
//! no network). Tests marked "database" run against the local Supabase
//! database named by `DEVTAB_TEST_DATABASE_URL`, acting as the API role, with
//! their own licences, users and plans, removed at the end; the others need
//! no database. Every answer is checked against the contract's schemas and
//! error table, every token against the published test key.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod support;

use std::num::NonZeroU32;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_MAX_AGE, ALLOW, CACHE_CONTROL, CONTENT_LENGTH,
    CONTENT_TYPE, HOST, ORIGIN, RETRY_AFTER, VARY,
};
use axum::http::{HeaderMap, Request, StatusCode};
use devtab_api::clock::ManualClock;
use devtab_api::config::RateLimits;
use devtab_api::licence::{key, token};
use devtab_api::store::licences::NewLicence;
use devtab_api::store::{customers, installations, licences};
use devtab_api::{Config, public_app};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use sqlx::PgConnection;
use support::schema::{assert_valid, load};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt as _;
use uuid::Uuid;

const OUR_ORIGIN: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";
const OTHER_ORIGIN: &str = "chrome-extension://pppppppppppppppppppppppppppppppp";

fn config() -> Config {
    Config {
        extension_origins: vec![OUR_ORIGIN.to_owned()],
        ..Config::default()
    }
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

async fn send(app: &Router, request: Request<Body>) -> Answer {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    Answer {
        status,
        headers,
        body,
    }
}

fn post(path: &str, body: &Value) -> Request<Body> {
    Request::post(path)
        .header(CONTENT_TYPE, "application/json")
        .header(ORIGIN, OUR_ORIGIN)
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// What the contract says of every answer of `endpoint`.
fn check_contract(endpoint: &str, answer: &Answer) {
    assert_eq!(answer.headers[CACHE_CONTROL], "no-store", "{endpoint}");
    match (endpoint, answer.status.as_u16()) {
        ("activate", 200) => assert_valid("activate.response.schema.json", &answer.body),
        ("refresh", 200) => assert_valid("refresh.response.schema.json", &answer.body),
        ("deactivate", 204) => assert!(answer.body.is_null(), "204 without a body"),
        (_, status) => {
            assert_valid("error.schema.json", &answer.body);
            let errors = load("errors.json");
            let code = answer.body["error"].as_str().unwrap();
            assert!(
                errors["endpoints"][endpoint]
                    .as_array()
                    .unwrap()
                    .contains(&Value::from(code)),
                "{endpoint} may not answer {code}"
            );
            assert_eq!(errors["codes"][code], status, "{code}");
            if status == 429 {
                let seconds = answer.headers[RETRY_AFTER].to_str().unwrap();
                assert!(
                    seconds.parse::<u64>().unwrap() >= 1,
                    "Retry-After {seconds}"
                );
            }
        }
    }
}

/// The verified claims of a token, checked against the token schemas.
fn claims(token: &str) -> Value {
    let keys = support::test_verifiers();
    let (header, claims) = token::verify(token, &keys).unwrap();
    assert_valid("token.header.schema.json", &header);
    assert_valid("token.claims.schema.json", &claims);
    claims
}

/// `(status, reason, chk, warn, exp)` with the times in days from `iat`.
fn summary(claims: &Value) -> (String, Option<String>, f64, f64, f64) {
    let iat = claims["iat"].as_i64().unwrap();
    #[allow(clippy::cast_precision_loss)]
    let days = |name: &str| (claims[name].as_i64().unwrap() - iat) as f64 / 86_400.0;
    (
        claims["status"].as_str().unwrap().to_owned(),
        claims["reason"].as_str().map(str::to_owned),
        days("chk"),
        days("warn"),
        days("exp"),
    )
}

fn inst(n: u8) -> String {
    format!("{:A<21}{}", "v1inst", char::from(b'a' + n))
}

#[derive(Clone, Copy)]
enum Spec {
    Subscription {
        plan: &'static str,
        paid_days: i64,
        renews: bool,
    },
    Complimentary {
        plan: &'static str,
        until_days: Option<i64>,
    },
}

struct Issued {
    id: Uuid,
    lid: String,
    key: String,
}

/// A database, a clock, the app and what the test created.
struct Fixture {
    owner: PgConnection,
    clock: ManualClock,
    app: Router,
    users: Vec<Uuid>,
    customers: Vec<Uuid>,
    licences: Vec<Uuid>,
}

impl Fixture {
    async fn new(config: Config) -> Option<Self> {
        let url = support::database_url()?;
        let mut owner = support::connect().await?;
        support::ensure_test_plans(&mut owner).await;
        let now = OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp())
            .unwrap();
        let clock = ManualClock::new(now);
        let state = support::api_state(&url, Arc::new(clock.clone()), &config).await;
        let app = public_app(&config, state.clone());
        Some(Self {
            owner,
            clock,
            app,
            users: Vec::new(),
            customers: Vec::new(),
            licences: Vec::new(),
        })
    }

    fn now(&self) -> OffsetDateTime {
        use devtab_api::clock::Clock as _;
        self.clock.now()
    }

    async fn issue(&mut self, spec: Spec) -> Issued {
        let lid = key::new_lid().unwrap();
        let new_key = key::generate().unwrap();
        let now = self.now();
        let (plan, kind, paid_until, renews, customer) = match spec {
            Spec::Subscription {
                plan,
                paid_days,
                renews,
            } => {
                let email = format!("v1-{}@example.invalid", Uuid::new_v4());
                let user = support::insert_auth_user(&mut self.owner, &email).await;
                self.users.push(user);
                let customer = customers::insert(
                    &mut self.owner,
                    user,
                    "test",
                    &format!("cus_TEST_{}", Uuid::new_v4()),
                )
                .await
                .unwrap();
                self.customers.push(customer);
                (
                    plan,
                    "subscription",
                    Some(now + Duration::days(paid_days)),
                    renews,
                    Some(customer),
                )
            }
            Spec::Complimentary { plan, until_days } => (
                plan,
                "complimentary",
                until_days.map(|d| now + Duration::days(d)),
                false,
                None,
            ),
        };
        let seats = sqlx::query_scalar::<_, i32>("select seats from app.plans where id = $1")
            .bind(plan)
            .fetch_one(&mut self.owner)
            .await
            .unwrap();
        let subscription_id = format!("sub_TEST_{}", Uuid::new_v4());
        let id = licences::insert(
            &mut self.owner,
            &NewLicence {
                members: 1,
                lid: &lid,
                user_id: None,
                customer_id: customer,
                plan_id: plan,
                kind,
                seats,
                paid_until,
                cancel_at_period_end: !renews,
                provider: customer.map(|_| "test"),
                provider_subscription_id: customer.map(|_| subscription_id.as_str()),
                key_hash: &new_key.hash,
                key_hint: &new_key.hint,
                note: None,
            },
        )
        .await
        .unwrap();
        self.licences.push(id);
        Issued {
            id,
            lid,
            key: new_key.display,
        }
    }

    async fn call(&self, endpoint: &str, body: &Value) -> Answer {
        let answer = send(&self.app, post(&format!("/v1/{endpoint}"), body)).await;
        check_contract(endpoint, &answer);
        answer
    }

    /// Activates and returns the claims and the secret.
    async fn activate(&self, key: &str, inst: &str) -> (Value, String) {
        let answer = self
            .call("activate", &json!({ "key": key, "inst": inst }))
            .await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        let secret = answer.body["secret"].as_str().unwrap().to_owned();
        (claims(answer.body["token"].as_str().unwrap()), secret)
    }

    async fn refresh(&self, lid: &str, inst: &str, secret: &str) -> Value {
        let answer = self
            .call(
                "refresh",
                &json!({ "lid": lid, "inst": inst, "secret": secret }),
            )
            .await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        let claims = claims(answer.body["token"].as_str().unwrap());
        assert_eq!(
            (claims["lid"].as_str(), claims["inst"].as_str()),
            (Some(lid), Some(inst))
        );
        claims
    }

    async fn deactivate(&self, lid: &str, inst: &str, secret: &str) {
        let answer = self
            .call(
                "deactivate",
                &json!({ "lid": lid, "inst": inst, "secret": secret }),
            )
            .await;
        assert_eq!(answer.status, StatusCode::NO_CONTENT);
    }

    async fn sql(&mut self, sql: impl sqlx::SqlSafeStr, id: Uuid) {
        sqlx::query(sql)
            .bind(id)
            .execute(&mut self.owner)
            .await
            .unwrap();
    }

    async fn installation_status(&mut self, licence: Uuid, inst: &str) -> (String, Option<String>) {
        sqlx::query_as("select status, released_reason from app.installations where licence_id = $1 and inst = $2")
            .bind(licence)
            .bind(inst)
            .fetch_one(&mut self.owner)
            .await
            .unwrap()
    }

    async fn cleanup(mut self) {
        for id in &self.licences {
            sqlx::query("delete from app.admin_audit where licence_id = $1")
                .bind(id)
                .execute(&mut self.owner)
                .await
                .unwrap();
            sqlx::query("delete from app.licences where id = $1")
                .bind(id)
                .execute(&mut self.owner)
                .await
                .unwrap();
        }
        for id in &self.customers {
            sqlx::query("delete from app.customers where id = $1")
                .bind(id)
                .execute(&mut self.owner)
                .await
                .unwrap();
        }
        for id in &self.users {
            sqlx::query("delete from auth.users where id = $1")
                .bind(id)
                .execute(&mut self.owner)
                .await
                .unwrap();
        }
    }
}

const MONTHLY: Spec = Spec::Subscription {
    plan: "test-monthly",
    paid_days: 30,
    renews: true,
};

// --- activation (database) ---------------------------------------------------

#[tokio::test]
async fn activation_gives_an_active_token_and_a_secret_and_repeating_it_takes_no_new_seat() {
    let Some(mut f) = Fixture::new(config()).await else {
        return;
    };
    let licence = f.issue(MONTHLY).await;
    let (first, secret) = f.activate(&licence.key, &inst(0)).await;
    assert_eq!(summary(&first), ("active".into(), None, 7.0, 23.0, 30.0));
    assert_eq!(first["lid"], licence.lid.as_str());
    assert_eq!(
        (first["term"].as_str(), first["renews"].as_bool()),
        (Some("month"), Some(true))
    );
    assert_eq!(first["iat"], f.now().unix_timestamp());
    assert_eq!(secret.len(), 43);

    // The key as a person may have typed it (the extension strips spaces):
    // lower case, O for 0, no hyphens.
    let typed = licence
        .key
        .to_lowercase()
        .replace('-', "")
        .replace('0', "o");
    let (again, new_secret) = f.activate(&typed, &inst(0)).await;
    assert_eq!(again["status"], "active");
    assert_ne!(secret, new_secret, "a new secret");
    let active = installations::count_active(&mut f.owner, licence.id)
        .await
        .unwrap();
    assert_eq!(active, 1, "the same seat");

    // The old secret no longer works; the new one does.
    let old = f.refresh(&licence.lid, &inst(0), &secret).await;
    assert_eq!(old["reason"], "unknown-installation");
    let fresh = f.refresh(&licence.lid, &inst(0), &new_secret).await;
    assert_eq!(fresh["status"], "active");
    f.cleanup().await;
}

#[tokio::test]
async fn activation_dates_follow_the_issuing_rule() {
    let Some(mut f) = Fixture::new(config()).await else {
        return;
    };
    for (spec, expected) in [
        (
            Spec::Subscription {
                plan: "test-monthly",
                paid_days: 10,
                renews: true,
            },
            (7.0, 17.0, 24.0),
        ),
        (
            Spec::Subscription {
                plan: "test-monthly",
                paid_days: -8,
                renews: true,
            },
            (6.0, 6.0, 6.0),
        ),
        (
            Spec::Complimentary {
                plan: "test-monthly",
                until_days: Some(3),
            },
            (3.0, 3.0, 3.0),
        ),
        (
            Spec::Complimentary {
                plan: "test-annual",
                until_days: None,
            },
            (7.0, 23.0, 30.0),
        ),
    ] {
        let licence = f.issue(spec).await;
        let (claims, _) = f.activate(&licence.key, &inst(1)).await;
        let (status, _, chk, warn, exp) = summary(&claims);
        assert_eq!(
            (status.as_str(), chk, warn, exp),
            ("active", expected.0, expected.1, expected.2)
        );
    }
    f.cleanup().await;
}

#[tokio::test]
async fn seats_run_out_and_a_deactivation_frees_one() {
    let Some(mut f) = Fixture::new(config()).await else {
        return;
    };
    let licence = f.issue(MONTHLY).await; // test-monthly: 2 seats
    let (_, secret_a) = f.activate(&licence.key, &inst(0)).await;
    f.activate(&licence.key, &inst(1)).await;
    let refused = f
        .call("activate", &json!({ "key": licence.key, "inst": inst(2) }))
        .await;
    assert_eq!(
        (refused.status, refused.body["error"].as_str()),
        (StatusCode::CONFLICT, Some("seat-limit"))
    );

    f.deactivate(&licence.lid, &inst(0), &secret_a).await;
    assert_eq!(
        f.installation_status(licence.id, &inst(0)).await.0,
        "deactivated"
    );
    f.activate(&licence.key, &inst(2)).await;
    f.cleanup().await;
}

#[tokio::test]
async fn too_many_new_activations_in_30_days_is_seat_limit_until_they_age() {
    let Some(mut f) = Fixture::new(config()).await else {
        return;
    };
    // One seat, churn 3: three new activations in 30 days.
    let licence = f
        .issue(Spec::Complimentary {
            plan: "test-single",
            until_days: None,
        })
        .await;
    for n in 0..3 {
        let (_, secret) = f.activate(&licence.key, &inst(n)).await;
        f.deactivate(&licence.lid, &inst(n), &secret).await;
        f.clock.advance(Duration::days(1));
    }
    let refused = f
        .call("activate", &json!({ "key": licence.key, "inst": inst(3) }))
        .await;
    assert_eq!(
        refused.body["error"], "seat-limit",
        "a free seat, but the fourth new activation"
    );

    f.clock.advance(Duration::days(28)); // the first activation is now 30 days old
    f.activate(&licence.key, &inst(3)).await;
    f.cleanup().await;
}

#[tokio::test]
async fn a_blocked_key_an_inactive_licence_and_an_unknown_key_are_refused() {
    let Some(mut f) = Fixture::new(config()).await else {
        return;
    };
    let blocked = f.issue(MONTHLY).await;
    let now = f.now();
    licences::block_key(&mut f.owner, blocked.id, now)
        .await
        .unwrap();
    let ended = f.issue(MONTHLY).await;
    f.sql(
        "update app.licences set status = 'ended', ended_at = now() where id = $1",
        ended.id,
    )
    .await;
    let refunded = f.issue(MONTHLY).await;
    licences::revoke_for_refund(&mut f.owner, refunded.id, now)
        .await
        .unwrap();
    let grace_over = f
        .issue(Spec::Subscription {
            plan: "test-monthly",
            paid_days: -15,
            renews: true,
        })
        .await;
    let comp_over = f
        .issue(Spec::Complimentary {
            plan: "test-monthly",
            until_days: Some(-1),
        })
        .await;

    for (key, status, code) in [
        (blocked.key.as_str(), StatusCode::FORBIDDEN, "key-blocked"),
        (
            ended.key.as_str(),
            StatusCode::FORBIDDEN,
            "licence-inactive",
        ),
        (
            refunded.key.as_str(),
            StatusCode::FORBIDDEN,
            "licence-inactive",
        ),
        (
            grace_over.key.as_str(),
            StatusCode::FORBIDDEN,
            "licence-inactive",
        ),
        (
            comp_over.key.as_str(),
            StatusCode::FORBIDDEN,
            "licence-inactive",
        ),
        (
            "DTD-0000-0000-0000-0000-0000",
            StatusCode::NOT_FOUND,
            "key-unknown",
        ),
        (
            "not-a-key-of-this-server",
            StatusCode::NOT_FOUND,
            "key-unknown",
        ),
    ] {
        let answer = f
            .call("activate", &json!({ "key": key, "inst": inst(0) }))
            .await;
        assert_eq!(
            (answer.status, answer.body["error"].as_str()),
            (status, Some(code)),
            "{key}"
        );
    }
    let taken = installations::count_active(&mut f.owner, blocked.id)
        .await
        .unwrap();
    assert_eq!(taken, 0);
    f.cleanup().await;
}

// --- refresh, every row of its table (database) ------------------------------

#[tokio::test]
async fn refresh_signs_every_row_of_its_table() {
    let Some(mut f) = Fixture::new(config()).await else {
        return;
    };
    let licence = f.issue(MONTHLY).await;
    let (_, secret) = f.activate(&licence.key, &inst(0)).await;

    // Unknown licence, unknown installation, wrong secret.
    let unknown_lid = "lic_0000000000000000000000TEST";
    let c = f.refresh(unknown_lid, &inst(0), &secret).await;
    assert_eq!(
        summary(&c),
        (
            "revoked".into(),
            Some("unknown-installation".into()),
            0.0,
            0.0,
            0.0
        )
    );
    assert!(c["paid"].is_null());
    let c = f.refresh(&licence.lid, &inst(9), &secret).await;
    assert_eq!(c["reason"], "unknown-installation");
    let c = f.refresh(&licence.lid, &inst(0), &"A".repeat(43)).await;
    assert_eq!(c["reason"], "unknown-installation");

    // Active: last seen now, the issuing rule.
    f.clock.advance(Duration::days(2));
    let c = f.refresh(&licence.lid, &inst(0), &secret).await;
    assert_eq!(
        summary(&c),
        ("active".into(), None, 7.0, 23.0, 30.0),
        "paid + grace is far off"
    );
    let seen: OffsetDateTime =
        sqlx::query_scalar("select last_seen_at from app.installations where licence_id = $1")
            .bind(licence.id)
            .fetch_one(&mut f.owner)
            .await
            .unwrap();
    assert_eq!(seen, f.now());

    // Released for want of contact, a seat free: back in active.
    f.sql(
        "update app.installations set status = 'released', released_reason = 'inactive', released_at = now() where licence_id = $1",
        licence.id,
    )
    .await;
    let c = f.refresh(&licence.lid, &inst(0), &secret).await;
    assert_eq!(c["status"], "active");
    assert_eq!(
        f.installation_status(licence.id, &inst(0)).await,
        ("active".into(), None)
    );

    // Released for want of contact, no seat free: seat-limit.
    f.sql(
        "update app.installations set status = 'released', released_reason = 'inactive', released_at = now() where licence_id = $1",
        licence.id,
    )
    .await;
    f.activate(&licence.key, &inst(1)).await;
    f.activate(&licence.key, &inst(2)).await;
    let c = f.refresh(&licence.lid, &inst(0), &secret).await;
    assert_eq!(
        summary(&c),
        ("revoked".into(), Some("seat-limit".into()), 0.0, 0.0, 0.0)
    );
    assert!(c.get("term").is_none() && c.get("renews").is_none());

    // Released by an administrator or from the site: deactivated.
    let other = f.issue(MONTHLY).await;
    let (_, other_secret) = f.activate(&other.key, &inst(0)).await;
    for reason in ["admin", "manual"] {
        f.sql(
            sqlx::AssertSqlSafe(format!(
                "update app.installations set status = 'released', released_reason = '{reason}', released_at = now() where licence_id = $1"
            )),
            other.id,
        )
        .await;
        let c = f.refresh(&other.lid, &inst(0), &other_secret).await;
        assert_eq!(c["reason"], "deactivated", "{reason}");
    }
    // Deactivated by the extension.
    let third = f.issue(MONTHLY).await;
    let (_, third_secret) = f.activate(&third.key, &inst(0)).await;
    f.deactivate(&third.lid, &inst(0), &third_secret).await;
    let c = f.refresh(&third.lid, &inst(0), &third_secret).await;
    assert_eq!(c["reason"], "deactivated");

    // Ended: `ended` with the term, renews false.
    let ended = f.issue(MONTHLY).await;
    let (_, ended_secret) = f.activate(&ended.key, &inst(0)).await;
    f.sql(
        "update app.licences set status = 'ended' where id = $1",
        ended.id,
    )
    .await;
    let c = f.refresh(&ended.lid, &inst(0), &ended_secret).await;
    assert_eq!(summary(&c), ("ended".into(), None, 7.0, 0.0, 0.0));
    assert_eq!(
        (c["term"].as_str(), c["renews"].as_bool()),
        (Some("month"), Some(false))
    );

    // Active in the database, the payment's grace over: `ended`.
    let late = f
        .issue(Spec::Subscription {
            plan: "test-monthly",
            paid_days: -13,
            renews: true,
        })
        .await;
    let (claims, late_secret) = f.activate(&late.key, &inst(0)).await;
    assert_eq!(summary(&claims), ("active".into(), None, 1.0, 1.0, 1.0));
    f.clock.advance(Duration::days(1));
    let c = f.refresh(&late.lid, &inst(0), &late_secret).await;
    assert_eq!(c["status"], "ended");

    // Refunded, then blocked: the key's block comes first.
    let refunded = f.issue(MONTHLY).await;
    let (_, refunded_secret) = f.activate(&refunded.key, &inst(0)).await;
    let now = f.now();
    licences::revoke_for_refund(&mut f.owner, refunded.id, now)
        .await
        .unwrap();
    let c = f.refresh(&refunded.lid, &inst(0), &refunded_secret).await;
    assert_eq!(c["reason"], "refund");
    assert!(c["paid"].is_i64(), "the paid date stays on revoked");
    licences::block_key(&mut f.owner, refunded.id, now)
        .await
        .unwrap();
    let c = f.refresh(&refunded.lid, &inst(0), &refunded_secret).await;
    assert_eq!(c["reason"], "key-blocked");
    f.cleanup().await;
}

// --- deactivation (database) -------------------------------------------------

#[tokio::test]
async fn deactivation_always_answers_204_and_changes_only_a_matching_installation() {
    let Some(mut f) = Fixture::new(config()).await else {
        return;
    };
    let licence = f.issue(MONTHLY).await;
    let (_, secret) = f.activate(&licence.key, &inst(0)).await;
    f.deactivate(&licence.lid, &inst(0), &"B".repeat(43)).await;
    assert_eq!(
        f.installation_status(licence.id, &inst(0)).await.0,
        "active",
        "wrong secret"
    );
    f.deactivate(&licence.lid, &inst(5), &secret).await;
    f.deactivate("lic_0000000000000000000000TEST", &inst(0), &secret)
        .await;
    f.deactivate(&licence.lid, &inst(0), &secret).await;
    assert_eq!(
        f.installation_status(licence.id, &inst(0)).await.0,
        "deactivated"
    );
    f.deactivate(&licence.lid, &inst(0), &secret).await; // again
    f.cleanup().await;
}

// --- rate limits (database for the body-keyed ones) --------------------------

#[tokio::test]
async fn rate_limits_answer_429_with_retry_after_in_seconds() {
    let quotas = RateLimits {
        activate_per_key: NonZeroU32::new(2).unwrap(),
        refresh_per_installation: NonZeroU32::new(1).unwrap(),
        ..RateLimits::default()
    };
    let Some(mut f) = Fixture::new(Config {
        rate_limits: quotas,
        ..config()
    })
    .await
    else {
        return;
    };
    let licence = f.issue(MONTHLY).await;
    let (_, secret) = f.activate(&licence.key, &inst(0)).await;
    f.activate(&licence.key, &inst(0)).await;
    let limited = f
        .call("activate", &json!({ "key": licence.key, "inst": inst(0) }))
        .await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.body["error"], "rate-limited");
    let seconds: u64 = limited.headers[RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1_790..=1_800).contains(&seconds), "two an hour: {seconds}");
    assert_eq!(
        limited.headers[ACCESS_CONTROL_ALLOW_ORIGIN], OUR_ORIGIN,
        "readable by the extension"
    );

    let body = json!({ "lid": licence.lid, "inst": inst(0), "secret": secret });
    assert_eq!(f.call("refresh", &body).await.status, StatusCode::OK);
    let limited = f.call("refresh", &body).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    f.cleanup().await;
}

#[tokio::test]
async fn the_address_limit_counts_malformed_requests_too() {
    let quotas = RateLimits {
        activate_per_ip: NonZeroU32::new(2).unwrap(),
        ..RateLimits::default()
    };
    let limited_config = Config {
        rate_limits: quotas,
        ..config()
    };
    let app_limited = public_app(&limited_config, support::offline_state(&limited_config));
    for _ in 0..2 {
        let answer = send(&app_limited, post("/v1/activate", &json!({}))).await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    }
    let answer = send(&app_limited, post("/v1/activate", &json!({}))).await;
    check_contract("activate", &answer);
    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
}

// --- without a database --------------------------------------------------------

fn offline(config: &Config) -> Router {
    public_app(config, support::offline_state(config))
}

#[tokio::test]
async fn a_preflight_from_our_origin_gets_exactly_the_cors_headers() {
    for path in ["/v1/activate", "/v1/refresh", "/v1/deactivate"] {
        let answer = send(
            &offline(&config()),
            Request::options(path)
                .header(ORIGIN, OUR_ORIGIN)
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "content-type")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(answer.status, StatusCode::NO_CONTENT, "{path}");
        assert!(answer.body.is_null());
        let h = &answer.headers;
        assert_eq!(h[ACCESS_CONTROL_ALLOW_ORIGIN], OUR_ORIGIN);
        assert_eq!(h[ACCESS_CONTROL_ALLOW_METHODS], "POST");
        assert_eq!(h[ACCESS_CONTROL_ALLOW_HEADERS], "Content-Type");
        assert_eq!(h[ACCESS_CONTROL_MAX_AGE], "86400");
        assert_eq!(h[VARY], "Origin");
        assert!(h.get(ACCESS_CONTROL_ALLOW_CREDENTIALS).is_none());
    }
}

#[tokio::test]
async fn another_origin_gets_no_cors_headers_and_is_still_answered() {
    let app = offline(&config());
    let answer = send(
        &app,
        Request::options("/v1/activate")
            .header(ORIGIN, OTHER_ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert!(answer.headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    assert_eq!(answer.headers[VARY], "Origin");

    let request = Request::post("/v1/activate")
        .header(CONTENT_TYPE, "application/json")
        .header(ORIGIN, OTHER_ORIGIN)
        .body(Body::from(r#"{"key":"short","inst":"x"}"#))
        .unwrap();
    let answer = send(&app, request).await;
    check_contract("activate", &answer);
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "handled as usual");
    assert!(answer.headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none());

    let request = Request::post("/v1/activate")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"key":"short","inst":"x"}"#))
        .unwrap();
    let answer = send(&app, request).await;
    assert_eq!(
        answer.status,
        StatusCode::BAD_REQUEST,
        "no Origin at all: handled as usual"
    );
}

#[tokio::test]
async fn bodies_off_the_contract_are_400() {
    let app = offline(&config());
    let good_inst = inst(0);
    let secret = "S".repeat(43);
    for (endpoint, body) in [
        ("activate", json!({ "key": "DTD-0000-0000-0000-0000-0000" })),
        (
            "activate",
            json!({ "key": "DTD-0000-0000-0000-0000-0000", "inst": good_inst, "extra": 1 }),
        ),
        (
            "activate",
            json!({ "key": "DTD-0000-0000-0000-0000-0000", "inst": "short" }),
        ),
        ("activate", json!({ "key": "too-short", "inst": good_inst })),
        (
            "activate",
            json!({ "key": "DTD 0000 0000 0000 0000 0000", "inst": good_inst }),
        ),
        ("activate", json!({ "key": 12, "inst": good_inst })),
        ("activate", json!([])),
        (
            "refresh",
            json!({ "lid": "lic_x", "inst": good_inst, "secret": secret }),
        ),
        (
            "refresh",
            json!({ "lid": "lic_0000000000000000000000TEST", "inst": good_inst, "secret": "short" }),
        ),
        (
            "refresh",
            json!({ "lid": "lic_0000000000000000000000TEST", "inst": good_inst }),
        ),
        (
            "deactivate",
            json!({ "lid": "lic_0000000000000000000000TEST", "inst": good_inst, "secret": secret, "x": 1 }),
        ),
        (
            "deactivate",
            json!({ "lid": "lic 0000000000", "inst": good_inst, "secret": secret }),
        ),
    ] {
        let answer = send(&app, post(&format!("/v1/{endpoint}"), &body)).await;
        check_contract(endpoint, &answer);
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{endpoint} {body}");
        assert_eq!(answer.body["error"], "bad-request");
    }
    let not_json = Request::post("/v1/activate")
        .header(ORIGIN, OUR_ORIGIN)
        .body(Body::from("key=x"))
        .unwrap();
    let answer = send(&app, not_json).await;
    check_contract("activate", &answer);
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_body_over_4_kib_is_413() {
    let app = offline(&config());
    let big = format!(r#"{{"key":"{}","inst":"x"}}"#, "k".repeat(4100));
    let declared = Request::post("/v1/activate")
        .header(CONTENT_TYPE, "application/json")
        .header(CONTENT_LENGTH, big.len())
        .body(Body::from(big.clone()))
        .unwrap();
    let answer = send(&app, declared).await;
    check_contract("activate", &answer);
    assert_eq!(answer.status, StatusCode::PAYLOAD_TOO_LARGE);
    let streamed = Request::post("/v1/refresh")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(big))
        .unwrap();
    let answer = send(&app, streamed).await;
    check_contract("refresh", &answer);
    assert_eq!(answer.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn a_wrong_method_is_405_with_allow_and_an_unknown_path_404() {
    let app = offline(&config());
    let answer = send(
        &app,
        Request::get("/v1/activate").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(answer.status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(answer.headers[ALLOW].to_str().unwrap().contains("POST"));
    assert_eq!(answer.body["error"], "method-not-allowed");
    let answer = send(&app, post("/v1/nothing", &json!({}))).await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND);
    assert_eq!(answer.body["error"], "not-found");
}

#[tokio::test]
async fn the_licence_endpoints_answer_only_on_the_licence_host() {
    let config = Config {
        licence_host: Some("licence.example.invalid".to_owned()),
        ..config()
    };
    let app = offline(&config);
    let mut request = post("/v1/activate", &json!({}));
    request
        .headers_mut()
        .insert(HOST, "web.example.invalid".parse().unwrap());
    let answer = send(&app, request).await;
    assert_eq!(
        (answer.status, answer.body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("not-found"))
    );
    let mut request = post("/v1/activate", &json!({}));
    request
        .headers_mut()
        .insert(HOST, "Licence.Example.Invalid:443".parse().unwrap());
    assert_eq!(send(&app, request).await.status, StatusCode::BAD_REQUEST);
    let health = Request::get("/api/healthz")
        .header(HOST, "web.example.invalid")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        send(&app, health).await.status,
        StatusCode::OK,
        "only /v1/* is checked"
    );
}

#[tokio::test]
async fn maintenance_is_503_with_retry_after_until_it_ends() {
    let config = Config {
        maintenance_until: Some(OffsetDateTime::now_utc() + Duration::minutes(10)),
        ..config()
    };
    let answer = send(&offline(&config), post("/v1/refresh", &json!({}))).await;
    check_contract("refresh", &answer);
    assert_eq!(
        (answer.status, answer.body["error"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("unavailable"))
    );
    let seconds: u64 = answer.headers[RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((590..=600).contains(&seconds), "{seconds}");
    assert_eq!(answer.headers[ACCESS_CONTROL_ALLOW_ORIGIN], OUR_ORIGIN);

    let over = Config {
        maintenance_until: Some(OffsetDateTime::now_utc() - Duration::minutes(1)),
        ..config
    };
    let answer = send(&offline(&over), post("/v1/refresh", &json!({}))).await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_database_that_does_not_answer_is_503_without_retry_after() {
    let answer = send(
        &offline(&config()),
        post(
            "/v1/activate",
            &json!({ "key": "DTD-0000-0000-0000-0000-0000", "inst": inst(0) }),
        ),
    )
    .await;
    check_contract("activate", &answer);
    assert_eq!(answer.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(answer.headers.get(RETRY_AFTER).is_none());
}
