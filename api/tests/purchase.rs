//! The purchase path against the test database: provider events turned into
//! licences (`process_event`), the webhook route, the queue's worker and the
//! checkout routes for the site.
//!
//! Stripe itself is never reached: `process_event` gets a provider double
//! whose subscription state the test sets (with Stripe's real mapping of
//! events to signals), and the routes that call Stripe's API get a stand-in
//! on a loopback port. Supabase Auth's admin API, which invites the buyers
//! of guest checkouts, is a stand-in too, which makes the invited user in
//! the test database as Auth would. Tests that need the database are skipped without
//! `DEVTAB_TEST_DATABASE_URL`; each makes its own plan, people and ids and
//! deletes them at the end.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use devtab_api::accounts::{Accounts, SecretKey};
use devtab_api::auth::Verifier;
use devtab_api::clock::{Clock as _, ManualClock, SharedClock};
use devtab_api::jobs::worker;
use devtab_api::licence::signer;
use devtab_api::payments::process::{self, ProcessError, Processed};
use devtab_api::payments::stripe::keys::{ApiKey, WebhookSecret};
use devtab_api::payments::stripe::{
    self, CheckoutUi, StripeMode, StripeProvider, StripeSettings, subscriptions,
};
use devtab_api::payments::{
    EventSource, PaymentSnapshot, Payments, ProviderError, ProviderFuture, Signal,
    SubscriptionSnapshot, SubscriptionStatus,
};
use devtab_api::store::{checkouts, events, jobs};
use devtab_api::{AppState, Config, internal_app, public_app};
use hmac::{Hmac, KeyInit as _, Mac as _};
use http_body_util::BodyExt as _;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{Connection as _, PgConnection, Row as _};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt as _;
use uuid::Uuid;

const ISSUER: &str = "http://127.0.0.1:54332/auth/v1";
const WEBHOOK_SECRET: &str = "whsec_purchasetests";

fn shared(clock: &ManualClock) -> SharedClock {
    Arc::new(clock.clone())
}

// ---- a provider double ----

/// Stripe's mapping of events, with subscription and payment states the test
/// sets.
#[derive(Debug, Default)]
struct Double {
    subscriptions: Mutex<Vec<SubscriptionSnapshot>>,
    payment: Mutex<Option<PaymentSnapshot>>,
    /// The address typed in each provider checkout, by its id.
    emails: Mutex<HashMap<String, String>>,
}

impl Double {
    fn set(&self, snapshot: SubscriptionSnapshot) {
        let mut all = self.subscriptions.lock().unwrap();
        all.retain(|s| s.id != snapshot.id);
        all.push(snapshot);
    }
}

impl EventSource for Double {
    fn id(&self) -> &'static str {
        stripe::ID
    }

    fn signals(&self, kind: &str, object: &Value) -> Vec<Signal> {
        subscriptions::signals(kind, object)
    }

    fn subscription<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, SubscriptionSnapshot> {
        Box::pin(async move {
            // Give a parallel event the chance to run in between.
            tokio::task::yield_now().await;
            self.subscriptions
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.id == id)
                .cloned()
                .ok_or_else(|| ProviderError::NotFound(format!("subscription {id}")))
        })
    }

    fn charge<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PaymentSnapshot> {
        Box::pin(async move {
            self.payment
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| ProviderError::NotFound(format!("charge {id}")))
        })
    }

    fn dispute<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PaymentSnapshot> {
        self.charge(id)
    }

    fn checkout_email<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, Option<String>> {
        Box::pin(async move { Ok(self.emails.lock().unwrap().get(id).cloned()) })
    }
}

// ---- the world of one test ----

struct World {
    url: String,
    owner: PgConnection,
    plan: String,
    price: String,
    people: Vec<Uuid>,
}

impl World {
    async fn new(available: bool) -> Option<Self> {
        Self::with(available, None).await
    }

    /// A world whose plan is a team plan of at least `min_members`, or a
    /// personal one.
    async fn with(available: bool, min_members: Option<i16>) -> Option<Self> {
        let url = support::database_url()?;
        let mut owner = PgConnection::connect(&url).await.unwrap();
        let tag = &Uuid::new_v4().simple().to_string()[..12];
        let plan = format!("buy-{tag}");
        let price = format!("price_{tag}");
        sqlx::query(
            r#"insert into app.plans (id, kind, "interval", seats, sort, available, min_members)
               values ($1, 'subscription', 'month', 2, 99, $2, $3)"#,
        )
        .bind(&plan)
        .bind(available)
        .bind(min_members)
        .execute(&mut owner)
        .await
        .unwrap();
        sqlx::query(
            "insert into app.plan_prices (plan_id, provider, provider_price_id, currency, amount_minor)
             values ($1, 'stripe', $2, 'EUR', 900)",
        )
        .bind(&plan)
        .bind(&price)
        .execute(&mut owner)
        .await
        .unwrap();
        Some(Self {
            url,
            owner,
            plan,
            price,
            people: Vec::new(),
        })
    }

    async fn person(&mut self) -> Uuid {
        let email = format!("buyer-{}@example.invalid", Uuid::new_v4().simple());
        let id = support::insert_auth_user(&mut self.owner, &email).await;
        self.people.push(id);
        id
    }

    async fn checkout(&mut self, user: Uuid, now: OffsetDateTime) -> Uuid {
        let price_id: Uuid = sqlx::query_scalar(
            "select id from app.plan_prices where provider = 'stripe' and provider_price_id = $1",
        )
        .bind(&self.price)
        .fetch_one(&mut self.owner)
        .await
        .unwrap();
        checkouts::insert(
            &mut self.owner,
            &checkouts::NewCheckout {
                user_id: Some(user),
                plan_id: &self.plan,
                price_id,
                provider: stripe::ID,
                locale: "en",
                created_at: now,
                expires_at: now + Duration::hours(24),
                claim_hash: None,
            },
        )
        .await
        .unwrap()
    }

    /// A guest checkout in `locale`, started at the provider as `cs_…`: its
    /// id and the provider's id of it.
    async fn guest_checkout(&mut self, locale: &str, now: OffsetDateTime) -> (Uuid, String) {
        let price_id: Uuid = sqlx::query_scalar(
            "select id from app.plan_prices where provider = 'stripe' and provider_price_id = $1",
        )
        .bind(&self.price)
        .fetch_one(&mut self.owner)
        .await
        .unwrap();
        let claim = devtab_api::licence::secret::generate().unwrap();
        let id = checkouts::insert(
            &mut self.owner,
            &checkouts::NewCheckout {
                user_id: None,
                plan_id: &self.plan,
                price_id,
                provider: stripe::ID,
                locale,
                created_at: now,
                expires_at: now + Duration::hours(24),
                claim_hash: Some(&claim.hash),
            },
        )
        .await
        .unwrap();
        let session = format!("cs_{}", Uuid::new_v4().simple());
        checkouts::set_provider_checkout(&mut self.owner, id, &session)
            .await
            .unwrap();
        (id, session)
    }

    /// An address nobody has, in mixed case and with spaces around, as a
    /// buyer may type it.
    fn new_email() -> String {
        format!(" Guest-{}@Example.Invalid ", Uuid::new_v4().simple())
    }

    fn snapshot(&self, id: &str, customer: &str, end: OffsetDateTime) -> SubscriptionSnapshot {
        SubscriptionSnapshot {
            id: id.to_owned(),
            status: SubscriptionStatus::Active,
            price_id: self.price.clone(),
            quantity: 1,
            current_period_end: end,
            cancel_at_period_end: false,
            customer: customer.to_owned(),
            period_paid: true,
            checkout: None,
        }
    }

    async fn event(&mut self, kind: &str, object: Value, now: OffsetDateTime) -> i64 {
        let event_id = format!("evt_{}", Uuid::new_v4().simple());
        let payload = json!({ "id": event_id, "type": kind, "data": { "object": object } });
        events::insert(
            &mut self.owner,
            &events::NewEvent {
                provider: stripe::ID,
                event_id: &event_id,
                event_type: kind,
                occurred_at: now,
                received_at: now,
                payload: &payload,
            },
        )
        .await
        .unwrap()
        .unwrap()
    }

    /// Processes an event in its own transaction as the API role.
    async fn process(
        &self,
        source: &dyn EventSource,
        event: i64,
        now: OffsetDateTime,
    ) -> Result<Processed, ProcessError> {
        self.process_with(source, None, event, now).await
    }

    /// The same, inviting guest buyers with `accounts`.
    async fn process_with(
        &self,
        source: &dyn EventSource,
        accounts: Option<&Accounts>,
        event: i64,
        now: OffsetDateTime,
    ) -> Result<Processed, ProcessError> {
        let pool = support::api_pool(&self.url).await;
        let mut tx = pool.begin().await.unwrap();
        let result = process::process_event(&mut tx, source, accounts, event, now).await;
        if result.is_ok() {
            tx.commit().await.unwrap();
        }
        result
    }

    async fn licences_of(&mut self, subscription: &str) -> Vec<sqlx::postgres::PgRow> {
        sqlx::query(
            "select id, user_id, status, revoke_reason, paid_until, past_due_since, cancel_at_period_end
             from app.licences where provider = 'stripe' and provider_subscription_id = $1",
        )
        .bind(subscription)
        .fetch_all(&mut self.owner)
        .await
        .unwrap()
    }

    async fn clean(mut self) {
        sqlx::query(
            "delete from app.jobs where kind = 'mail_licence' and (payload->>'licence')::uuid in
               (select id from app.licences where plan_id = $1)",
        )
        .bind(&self.plan)
        .execute(&mut self.owner)
        .await
        .unwrap();
        sqlx::query(
            "delete from app.licence_members where licence_id in
               (select id from app.licences where plan_id = $1)",
        )
        .bind(&self.plan)
        .execute(&mut self.owner)
        .await
        .unwrap();
        sqlx::query("delete from app.licences where plan_id = $1")
            .bind(&self.plan)
            .execute(&mut self.owner)
            .await
            .unwrap();
        for id in &self.people {
            sqlx::query("delete from app.customers where user_id = $1")
                .bind(id)
                .execute(&mut self.owner)
                .await
                .unwrap();
            sqlx::query("delete from auth.users where id = $1")
                .bind(id)
                .execute(&mut self.owner)
                .await
                .unwrap();
        }
        sqlx::query("delete from app.checkouts where plan_id = $1")
            .bind(&self.plan)
            .execute(&mut self.owner)
            .await
            .unwrap();
        sqlx::query("delete from app.plan_prices where plan_id = $1")
            .bind(&self.plan)
            .execute(&mut self.owner)
            .await
            .unwrap();
        sqlx::query("delete from app.plans where id = $1")
            .bind(&self.plan)
            .execute(&mut self.owner)
            .await
            .unwrap();
    }
}

fn session(checkout: Uuid, customer: &str, subscription: &str) -> Value {
    json!({
        "id": format!("cs_{}", Uuid::new_v4().simple()), "object": "checkout.session",
        "mode": "subscription", "client_reference_id": checkout.to_string(),
        "customer": customer, "subscription": subscription,
    })
}

fn paid_invoice(subscription: &str) -> Value {
    json!({
        "id": format!("in_{}", Uuid::new_v4().simple()),
        "parent": { "type": "subscription_details",
                    "subscription_details": { "subscription": subscription } },
    })
}

fn ids() -> (String, String) {
    let tag = Uuid::new_v4().simple().to_string();
    (format!("sub_{tag}"), format!("cus_{tag}"))
}

// ---- process_event ----

#[tokio::test]
async fn a_checkout_and_its_invoice_at_once_issue_one_licence_to_the_buyer() {
    let Some(mut world) = World::new(false).await else {
        return;
    };
    let now = OffsetDateTime::now_utc();
    let buyer = world.person().await;
    let checkout = world.checkout(buyer, now).await;
    let (sub, cus) = ids();
    let double = Double::default();
    let mut snapshot = world.snapshot(&sub, &cus, now + Duration::days(30));
    snapshot.checkout = Some(checkout);
    double.set(snapshot);

    let completed = world
        .event(
            "checkout.session.completed",
            session(checkout, &cus, &sub),
            now,
        )
        .await;
    let paid = world.event("invoice.paid", paid_invoice(&sub), now).await;
    let (a, b) = tokio::join!(
        world.process(&double, completed, now),
        world.process(&double, paid, now)
    );
    assert_eq!(
        (a.unwrap(), b.unwrap()),
        (Processed::Processed, Processed::Processed)
    );

    let licences = world.licences_of(&sub).await;
    assert_eq!(
        licences.len(),
        1,
        "one licence for two events of one purchase"
    );
    let licence: Uuid = licences[0].get("id");
    assert_eq!(licences[0].get::<Option<Uuid>, _>("user_id"), Some(buyer));
    assert_eq!(licences[0].get::<String, _>("status"), "active");
    let row = sqlx::query("select status, licence_id from app.checkouts where id = $1")
        .bind(checkout)
        .fetch_one(&mut world.owner)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "completed");
    assert_eq!(row.get::<Option<Uuid>, _>("licence_id"), Some(licence));
    let reveals: i64 =
        sqlx::query_scalar("select count(*) from app.key_reveals where licence_id = $1")
            .bind(licence)
            .fetch_one(&mut world.owner)
            .await
            .unwrap();
    assert_eq!(reveals, 1);

    // The same event again does nothing.
    assert_eq!(
        world.process(&double, paid, now).await.unwrap(),
        Processed::AlreadyDone
    );
    world.clean().await;
}

#[tokio::test]
async fn renewals_overdue_payments_and_refunds_follow_the_subscription() {
    let Some(mut world) = World::new(false).await else {
        return;
    };
    let now = OffsetDateTime::now_utc();
    let buyer = world.person().await;
    let checkout = world.checkout(buyer, now).await;
    let (sub, cus) = ids();
    let double = Double::default();
    let first_end = now + Duration::days(30);
    double.set(world.snapshot(&sub, &cus, first_end));
    let event = world
        .event(
            "checkout.session.completed",
            session(checkout, &cus, &sub),
            now,
        )
        .await;
    world.process(&double, event, now).await.unwrap();

    // A new period whose invoice is not paid yet: paid_until stays.
    let later = now + Duration::days(30);
    let mut renewing = world.snapshot(&sub, &cus, first_end + Duration::days(30));
    renewing.period_paid = false;
    double.set(renewing.clone());
    let event = world
        .event("customer.subscription.updated", json!({"id": sub}), later)
        .await;
    world.process(&double, event, later).await.unwrap();
    let row = &world.licences_of(&sub).await[0];
    assert_eq!(
        row.get::<Option<OffsetDateTime>, _>("paid_until")
            .map(OffsetDateTime::unix_timestamp),
        Some(first_end.unix_timestamp())
    );

    // The charge fails: overdue from now, still active, still the old period.
    renewing.status = SubscriptionStatus::PastDue;
    double.set(renewing.clone());
    let event = world
        .event("invoice.payment_failed", paid_invoice(&sub), later)
        .await;
    world.process(&double, event, later).await.unwrap();
    let row = &world.licences_of(&sub).await[0];
    assert_eq!(row.get::<String, _>("status"), "active");
    assert!(
        row.get::<Option<OffsetDateTime>, _>("past_due_since")
            .is_some()
    );

    // Paid at last.
    renewing.status = SubscriptionStatus::Active;
    renewing.period_paid = true;
    double.set(renewing.clone());
    let event = world.event("invoice.paid", paid_invoice(&sub), later).await;
    world.process(&double, event, later).await.unwrap();
    let row = &world.licences_of(&sub).await[0];
    assert_eq!(
        row.get::<Option<OffsetDateTime>, _>("paid_until")
            .map(OffsetDateTime::unix_timestamp),
        Some(renewing.current_period_end.unix_timestamp())
    );
    assert!(
        row.get::<Option<OffsetDateTime>, _>("past_due_since")
            .is_none()
    );

    // A partial refund changes nothing; a full one revokes.
    *double.payment.lock().unwrap() = Some(PaymentSnapshot {
        subscription: Some(sub.clone()),
        refunded_fully: false,
        dispute_lost: false,
    });
    let event = world
        .event("charge.refunded", json!({"id": "ch_1"}), later)
        .await;
    world.process(&double, event, later).await.unwrap();
    assert_eq!(
        world.licences_of(&sub).await[0].get::<String, _>("status"),
        "active"
    );
    *double.payment.lock().unwrap() = Some(PaymentSnapshot {
        subscription: Some(sub.clone()),
        refunded_fully: true,
        dispute_lost: false,
    });
    let event = world
        .event("charge.refunded", json!({"id": "ch_1"}), later)
        .await;
    world.process(&double, event, later).await.unwrap();
    let row = &world.licences_of(&sub).await[0];
    assert_eq!(row.get::<String, _>("status"), "revoked");
    assert_eq!(
        row.get::<Option<String>, _>("revoke_reason").as_deref(),
        Some("refund")
    );

    // Ignored events are marked so.
    let event = world
        .event("customer.updated", json!({"id": cus}), later)
        .await;
    assert_eq!(
        world.process(&double, event, later).await.unwrap(),
        Processed::Ignored
    );
    world.clean().await;
}

#[tokio::test]
async fn a_customer_of_another_account_or_a_paid_subscription_without_our_checkout_stops() {
    let Some(mut world) = World::new(false).await else {
        return;
    };
    let now = OffsetDateTime::now_utc();
    let (first, second) = (world.person().await, world.person().await);
    let double = Double::default();
    let (sub, cus) = ids();
    let checkout = world.checkout(first, now).await;
    double.set(world.snapshot(&sub, &cus, now + Duration::days(30)));
    let event = world
        .event(
            "checkout.session.completed",
            session(checkout, &cus, &sub),
            now,
        )
        .await;
    world.process(&double, event, now).await.unwrap();

    // The second person's checkout comes back with the first one's customer.
    let (other_sub, _) = ids();
    let checkout = world.checkout(second, now).await;
    double.set(world.snapshot(&other_sub, &cus, now + Duration::days(30)));
    let event = world
        .event(
            "checkout.session.completed",
            session(checkout, &cus, &other_sub),
            now,
        )
        .await;
    let err = world.process(&double, event, now).await.unwrap_err();
    assert!(
        matches!(&err, ProcessError::Permanent(m) if m.contains("another account")),
        "{err}"
    );
    assert!(world.licences_of(&other_sub).await.is_empty());

    // A paid subscription nobody started here.
    let (stray, stray_cus) = ids();
    double.set(world.snapshot(&stray, &stray_cus, now + Duration::days(30)));
    let event = world
        .event("customer.subscription.created", json!({"id": stray}), now)
        .await;
    let err = world.process(&double, event, now).await.unwrap_err();
    assert!(
        matches!(&err, ProcessError::Permanent(m) if m.contains("no checkout")),
        "{err}"
    );
    world.clean().await;
}

// ---- Stripe's API, stood in on a loopback port ----

#[derive(Debug, Default)]
struct StandIn {
    subscription: Mutex<Option<Value>>,
    checkouts: Mutex<Vec<Vec<(String, String)>>>,
    /// The address every checkout session says its buyer typed.
    email: Mutex<Option<String>>,
}

async fn stand_in() -> (String, Arc<StandIn>) {
    let state = Arc::new(StandIn::default());
    let app = Router::new()
        .route(
            "/v1/subscriptions/{id}",
            get(|State(s): State<Arc<StandIn>>, Path(_id): Path<String>| async move {
                Json(s.subscription.lock().unwrap().clone().unwrap_or(Value::Null))
            }),
        )
        .route(
            "/v1/checkout/sessions",
            post(
                |State(s): State<Arc<StandIn>>,
                 axum::Form(form): axum::Form<Vec<(String, String)>>| async move {
                    s.checkouts.lock().unwrap().push(form);
                    let id = format!("cs_test_{}", Uuid::new_v4().simple());
                    Json(json!({"id": id, "url": "https://checkout.stripe.com/c/pay/cs_test_standin"}))
                },
            ),
        )
        .route(
            "/v1/checkout/sessions/{id}",
            get(|State(s): State<Arc<StandIn>>, Path(id): Path<String>| async move {
                let email = s.email.lock().unwrap().clone();
                Json(json!({"id": id, "object": "checkout.session", "customer_details": {"email": email}}))
            }),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), state)
}

fn payments(api_url: &str) -> Payments {
    let settings = StripeSettings {
        mode: StripeMode::Sandbox,
        api_key_file: "unused".into(),
        webhook_secret_file: None,
        managed_payments: true,
        automatic_tax: false,
        checkout_ui: CheckoutUi::Hosted,
        publishable_key: None,
        api_url: api_url.to_owned(),
        api_version: stripe::DEFAULT_API_VERSION.to_owned(),
        timeout_s: 5,
        webhook_tolerance_s: 300,
    };
    let key = ApiKey::parse("rk_test_purchase", StripeMode::Sandbox).unwrap();
    Payments {
        stripe: StripeProvider::with_key(settings, key)
            .unwrap()
            .with_retry_delays(Vec::new()),
        webhook_secret: WebhookSecret::parse(WEBHOOK_SECRET).unwrap(),
    }
}

fn signed(body: &str, at: OffsetDateTime) -> String {
    let t = at.unix_timestamp();
    let mut mac = Hmac::<Sha256>::new_from_slice(WEBHOOK_SECRET.as_bytes()).unwrap();
    mac.update(format!("{t}.{body}").as_bytes());
    let signature = mac
        .finalize()
        .into_bytes()
        .iter()
        .fold(String::new(), |mut hex, b| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{b:02x}");
            hex
        });
    format!("t={t},v1={signature}")
}

async fn deliver(
    state: AppState,
    provider: &str,
    body: &str,
    signature: &str,
) -> (StatusCode, Value) {
    let response = public_app(&Config::default(), state)
        .oneshot(
            Request::post(format!("/api/webhooks/{provider}"))
                .header("stripe-signature", signature)
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn webhooks_are_refused_without_a_provider_a_known_name_or_a_matching_signature() {
    let clock = ManualClock::new(OffsetDateTime::now_utc());
    let body = r#"{"id":"evt_x","type":"customer.updated","created":1,"livemode":false,"data":{"object":{}}}"#;
    let mut offline = support::offline_state(&Config::default());
    offline.clock = shared(&clock);
    let (status, _) = deliver(offline.clone(), "stripe", body, &signed(body, clock.now())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no provider configured");

    let state = offline.with_payments(payments("http://127.0.0.1:9"));
    let (status, _) = deliver(state.clone(), "paypal", body, &signed(body, clock.now())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, answer) = deliver(state.clone(), "stripe", body, "t=1,v1=00").await;
    assert_eq!(
        (status, answer["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("signature"))
    );
    let old_signature = signed(body, clock.now() - Duration::minutes(10));
    let (status, _) = deliver(state, "stripe", body, &old_signature).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_webhook_is_stored_once_and_the_worker_issues_the_licence() {
    let Some(mut world) = World::new(false).await else {
        return;
    };
    let clock = ManualClock::new(OffsetDateTime::now_utc());
    let now = clock.now();
    let buyer = world.person().await;
    let checkout = world.checkout(buyer, now).await;
    let (sub, cus) = ids();
    let (api, stand) = stand_in().await;
    *stand.subscription.lock().unwrap() = Some(json!({
        "id": sub, "status": "active", "customer": cus, "cancel_at_period_end": false,
        "metadata": {"devtab_checkout": checkout.to_string()},
        "items": {"data": [{"price": {"id": world.price}, "current_period_end": (now + Duration::days(30)).unix_timestamp()}]},
        "latest_invoice": {"id": "in_1", "status": "paid"},
    }));
    let state = support::api_state(&world.url, shared(&clock), &Config::default())
        .await
        .with_payments(payments(&api));
    let event_id = format!("evt_{}", Uuid::new_v4().simple());
    let body = json!({
        "id": event_id, "object": "event", "type": "checkout.session.completed",
        "created": now.unix_timestamp(), "api_version": stripe::DEFAULT_API_VERSION,
        "livemode": false, "data": {"object": session(checkout, &cus, &sub)},
    })
    .to_string();
    for _ in 0..2 {
        let (status, answer) = deliver(state.clone(), "stripe", &body, &signed(&body, now)).await;
        assert_eq!(
            (status, answer),
            (StatusCode::OK, json!({"received": true}))
        );
    }
    let stored: i64 =
        sqlx::query_scalar("select count(*) from app.provider_events where event_id = $1")
            .bind(&event_id)
            .fetch_one(&mut world.owner)
            .await
            .unwrap();
    assert_eq!(stored, 1, "a repeated delivery is stored once");

    assert!(worker::run_due(&state).await.unwrap() >= 1);
    let status: String =
        sqlx::query_scalar("select status from app.provider_events where event_id = $1")
            .bind(&event_id)
            .fetch_one(&mut world.owner)
            .await
            .unwrap();
    assert_eq!(status, "processed");
    assert_eq!(world.licences_of(&sub).await.len(), 1);
    world.clean().await;
}

// ---- the checkout routes ----

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
    Keys {
        jwk: json!({
            "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig",
            "kid": entry.kid, "x": entry.jwk.x, "y": entry.jwk.y,
        }),
        kid: entry.kid,
        encoding: EncodingKey::from_ec_der(&STANDARD.decode(body).unwrap()),
    }
}

fn token(keys: &Keys, user: Uuid, clock: &ManualClock) -> String {
    let claims = json!({
        "sub": user.to_string(), "aud": "authenticated", "iss": ISSUER,
        "exp": (clock.now() + Duration::hours(1)).unix_timestamp(), "role": "authenticated",
    });
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(keys.kid.clone());
    jsonwebtoken::encode(&header, &claims, &keys.encoding).unwrap()
}

async fn call(
    state: AppState,
    method: &str,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = internal_app(&Config::default(), state)
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_signed_in_person_starts_a_checkout_and_alone_sees_its_key_once() {
    let Some(mut world) = World::new(true).await else {
        return;
    };
    let clock = ManualClock::new(OffsetDateTime::now_utc());
    let keys = keys();
    let (api, stand) = stand_in().await;
    let state = support::api_state(&world.url, shared(&clock), &Config::default())
        .await
        .with_auth(Verifier::fixed(
            &json!({"keys": [keys.jwk]}),
            ISSUER.to_owned(),
            shared(&clock),
        ))
        .with_payments(payments(&api));
    let (buyer, someone) = (world.person().await, world.person().await);
    let (buyer_token, someone_token) = (token(&keys, buyer, &clock), token(&keys, someone, &clock));

    let (status, body) = call(
        state.clone(),
        "POST",
        "/internal/checkouts",
        &buyer_token,
        Some(json!({"plan": "no-such-plan"})),
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("plan-unknown"))
    );
    let (status, body) = call(
        state.clone(),
        "POST",
        "/internal/checkouts",
        &buyer_token,
        Some(json!({"plan": world.plan, "locale": "ru"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        body["redirect_url"],
        "https://checkout.stripe.com/c/pay/cs_test_standin"
    );
    assert!(
        body.get("claim").is_none(),
        "a signed-in purchase answers as before"
    );
    let checkout = Uuid::parse_str(body["checkout"].as_str().unwrap()).unwrap();
    {
        let sent = stand.checkouts.lock().unwrap();
        let form = &sent[0];
        let value = |k: &str| {
            form.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(value("line_items[0][price]"), Some(world.price.as_str()));
        assert_eq!(
            value("client_reference_id"),
            Some(checkout.to_string().as_str())
        );
        assert_eq!(
            value("success_url"),
            Some(format!("https://web.example.invalid/ru/pro/thanks/?ref={checkout}").as_str())
        );
        assert_eq!(value("locale"), Some("ru"));
    }

    let path = format!("/internal/checkouts/{checkout}");
    let (status, body) = call(state.clone(), "GET", &path, &someone_token, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "someone else's checkout: {body}"
    );
    let (status, body) = call(state.clone(), "GET", &path, &buyer_token, None).await;
    assert_eq!(
        (status, body),
        (
            StatusCode::OK,
            json!({"status": "open", "key": null, "key_hint": null})
        )
    );

    // The payment arrives.
    let (sub, cus) = ids();
    let double = Double::default();
    double.set(world.snapshot(&sub, &cus, clock.now() + Duration::days(30)));
    let event = world
        .event(
            "checkout.session.completed",
            session(checkout, &cus, &sub),
            clock.now(),
        )
        .await;
    world.process(&double, event, clock.now()).await.unwrap();

    let (status, body) = call(state.clone(), "GET", &path, &buyer_token, None).await;
    assert_eq!(
        (status, &body["status"]),
        (StatusCode::OK, &json!("completed"))
    );
    let key = body["key"].as_str().expect("the key, the first time");
    assert!(key.starts_with("DTD-"));
    assert_eq!(body["key_hint"].as_str(), key.get(key.len() - 4..));
    let (_, again) = call(state, "GET", &path, &buyer_token, None).await;
    assert_eq!(again["key"], Value::Null, "only once");
    assert_eq!(again["key_hint"], body["key_hint"]);
    world.clean().await;
}

#[tokio::test]
async fn a_plan_not_on_sale_and_no_provider_are_refused() {
    let Some(mut world) = World::new(false).await else {
        return;
    };
    let clock = ManualClock::new(OffsetDateTime::now_utc());
    let keys = keys();
    let verifier = || {
        Verifier::fixed(
            &json!({"keys": [keys.jwk]}),
            ISSUER.to_owned(),
            shared(&clock),
        )
    };
    let buyer = world.person().await;
    let bearer = token(&keys, buyer, &clock);
    let base = support::api_state(&world.url, shared(&clock), &Config::default()).await;

    let (status, body) = call(
        base.clone().with_auth(verifier()),
        "POST",
        "/internal/checkouts",
        &bearer,
        Some(json!({"plan": world.plan})),
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("payments-unavailable")
        )
    );
    let state = base
        .with_auth(verifier())
        .with_payments(payments("http://127.0.0.1:9"));
    let (status, body) = call(
        state,
        "POST",
        "/internal/checkouts",
        &bearer,
        Some(json!({"plan": world.plan})),
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("plan-unavailable"))
    );
    world.clean().await;
}

// ---- connecting the extension from the site ----

async fn active_seats(owner: &mut PgConnection, lid: &str) -> i64 {
    sqlx::query_scalar(
        "select count(*) from app.installations i join app.licences l on l.id = i.licence_id
         where l.lid = $1 and i.status = 'active'",
    )
    .bind(lid)
    .fetch_one(owner)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_site_connects_the_signed_in_person_s_extension_without_a_key() {
    let Some(mut world) = World::new(false).await else {
        return;
    };
    let clock = ManualClock::new(OffsetDateTime::now_utc());
    let keys = keys();
    let state = support::api_state(&world.url, shared(&clock), &Config::default())
        .await
        .with_auth(Verifier::fixed(
            &json!({"keys": [keys.jwk]}),
            ISSUER.to_owned(),
            shared(&clock),
        ));
    let (buyer, nobody) = (world.person().await, world.person().await);
    let (buyer_token, nobody_token) = (token(&keys, buyer, &clock), token(&keys, nobody, &clock));
    let inst = "AAAAAAAAAAAAAAAAAAAAAA";
    let connect = |token: String, body: Value| {
        let state = state.clone();
        async move {
            call(
                state,
                "POST",
                "/internal/licences/connect",
                &token,
                Some(body),
            )
            .await
        }
    };

    let (status, body) = connect(nobody_token.clone(), json!({"inst": inst})).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("no-licence"))
    );
    let (status, _) = connect(buyer_token.clone(), json!({"inst": "short"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The buyer pays for a subscription.
    let double = Double::default();
    let mut lids = Vec::new();
    for _ in 0..2 {
        let checkout = world.checkout(buyer, clock.now()).await;
        let (sub, _) = ids();
        let cus = format!("cus_{buyer}").replace('-', "");
        double.set(world.snapshot(&sub, &cus, clock.now() + Duration::days(30)));
        let event = world
            .event(
                "checkout.session.completed",
                session(checkout, &cus, &sub),
                clock.now(),
            )
            .await;
        world.process(&double, event, clock.now()).await.unwrap();
        let lid: String =
            sqlx::query_scalar("select lid from app.licences where provider_subscription_id = $1")
                .bind(&sub)
                .fetch_one(&mut world.owner)
                .await
                .unwrap();
        lids.push(lid);
        if lids.len() == 1 {
            // One licence: connected without naming it.
            let (status, body) = connect(buyer_token.clone(), json!({"inst": inst})).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["lid"], lids[0]);
            let (_, claims) = devtab_api::licence::token::verify(
                body["token"].as_str().unwrap(),
                &support::test_verifiers(),
            )
            .unwrap();
            assert_eq!(
                (claims["inst"].as_str(), claims["status"].as_str()),
                (Some(inst), Some("active"))
            );
            assert!(body["secret"].as_str().unwrap().len() >= 32);
        }
    }
    // Two licences: the browser already on one stays on it; a new browser goes
    // to the longest-running; a licence can still be named; someone else's is not found.
    let (status, body) = connect(buyer_token.clone(), json!({"inst": inst})).await;
    assert_eq!(
        (status, &body["lid"]),
        (StatusCode::OK, &json!(lids[0])),
        "{body}"
    );
    let (status, body) = connect(
        buyer_token.clone(),
        json!({"inst": "BBBBBBBBBBBBBBBBBBBBBB"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(lids.contains(&body["lid"].as_str().unwrap().to_owned()));
    let (status, body) = connect(buyer_token.clone(), json!({"inst": inst, "lid": lids[1]})).await;
    assert_eq!((status, &body["lid"]), (StatusCode::OK, &json!(lids[1])));
    let (status, _) = connect(nobody_token, json!({"inst": inst, "lid": lids[1]})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Connecting the same browser again takes no second seat.
    for _ in 0..2 {
        connect(buyer_token.clone(), json!({"inst": inst, "lid": lids[0]})).await;
    }
    assert_eq!(active_seats(&mut world.owner, &lids[0]).await, 1);
    world.clean().await;
}

// ---- team licences ----

/// The licence `owner` bought for `quantity`: its lid, subscription and the
/// subscription's state, as the double holds it.
async fn bought(
    world: &mut World,
    double: &Double,
    owner: Uuid,
    quantity: u32,
    now: OffsetDateTime,
) -> (String, String, SubscriptionSnapshot) {
    let checkout = world.checkout(owner, now).await;
    let (sub, cus) = ids();
    let mut snapshot = world.snapshot(&sub, &cus, now + Duration::days(30));
    snapshot.quantity = quantity;
    double.set(snapshot.clone());
    let event = world
        .event(
            "checkout.session.completed",
            session(checkout, &cus, &sub),
            now,
        )
        .await;
    world.process(double, event, now).await.unwrap();
    let lid: String =
        sqlx::query_scalar("select lid from app.licences where provider_subscription_id = $1")
            .bind(&sub)
            .fetch_one(&mut world.owner)
            .await
            .unwrap();
    (lid, sub, snapshot)
}

async fn size_of(world: &mut World, lid: &str) -> (i32, i32) {
    sqlx::query_as("select members, seats from app.licences where lid = $1")
        .bind(lid)
        .fetch_one(&mut world.owner)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_team_is_bought_per_member_and_its_owner_invites_who_connects() {
    let Some(mut world) = World::with(false, Some(2)).await else {
        return;
    };
    let clock = ManualClock::new(OffsetDateTime::now_utc());
    let keys = keys();
    let state = support::api_state(&world.url, shared(&clock), &Config::default())
        .await
        .with_auth(Verifier::fixed(
            &json!({"keys": [keys.jwk]}),
            ISSUER.to_owned(),
            shared(&clock),
        ));
    let owner = world.person().await;
    let member_email = format!("member-{}@example.invalid", Uuid::new_v4().simple());
    let member = support::insert_auth_user(&mut world.owner, &member_email).await;
    world.people.push(member);
    let stranger = world.person().await;
    let (owner_token, member_token, stranger_token) = (
        token(&keys, owner, &clock),
        token(&keys, member, &clock),
        token(&keys, stranger, &clock),
    );

    // Bought for three members: the plan's two browsers each.
    let double = Double::default();
    let (lid, _, _) = bought(&mut world, &double, owner, 3, clock.now()).await;
    assert_eq!(size_of(&mut world, &lid).await, (3, 6));
    let path = format!("/internal/licences/{lid}/members");
    let invite = |token: String, email: String| {
        let (state, path) = (state.clone(), path.clone());
        async move { call(state, "POST", &path, &token, Some(json!({"email": email}))).await }
    };
    // The owner and two invited: the third address does not fit.
    assert_eq!(
        invite(owner_token.clone(), member_email.to_uppercase())
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        invite(owner_token.clone(), "second@example.invalid".to_owned())
            .await
            .0,
        StatusCode::CREATED
    );
    let (status, body) = invite(owner_token.clone(), "third@example.invalid".to_owned()).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("team-full"))
    );
    assert_eq!(
        invite(stranger_token.clone(), "x@example.invalid".to_owned())
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // The invited person connects from their account.
    let inst = "MMMMMMMMMMMMMMMMMMMMMM";
    let (status, body) = call(
        state.clone(),
        "POST",
        "/internal/licences/connect",
        &member_token,
        Some(json!({"inst": inst})),
    )
    .await;
    assert_eq!(
        (status, &body["lid"]),
        (StatusCode::OK, &json!(lid)),
        "{body}"
    );
    let (status, team) = call(state.clone(), "GET", &path, &owner_token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(team["members"], 3);
    assert_eq!(
        team["invited"][0],
        json!({"email": member_email, "joined": true})
    );
    assert_eq!(
        team["invited"][1],
        json!({"email": "second@example.invalid", "joined": false})
    );
    let (status, _) = call(
        state.clone(),
        "POST",
        "/internal/licences/connect",
        &stranger_token,
        Some(json!({"inst": "SSSSSSSSSSSSSSSSSSSSSS"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "not invited: no licence");

    // Removed: their browser is disconnected.
    assert_eq!(active_seats(&mut world.owner, &lid).await, 1);
    let (status, _) = call(
        state.clone(),
        "DELETE",
        &format!("{path}/{member_email}"),
        &owner_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(active_seats(&mut world.owner, &lid).await, 0);

    world.clean().await;
}

#[tokio::test]
async fn a_personal_licence_has_no_members() {
    let Some(mut world) = World::new(false).await else {
        return;
    };
    let clock = ManualClock::new(OffsetDateTime::now_utc());
    let keys = keys();
    let state = support::api_state(&world.url, shared(&clock), &Config::default())
        .await
        .with_auth(Verifier::fixed(
            &json!({"keys": [keys.jwk]}),
            ISSUER.to_owned(),
            shared(&clock),
        ));
    let owner = world.person().await;
    let double = Double::default();
    let (lid, _, _) = bought(&mut world, &double, owner, 5, clock.now()).await;
    assert_eq!(
        size_of(&mut world, &lid).await,
        (1, 2),
        "a person, whatever the quantity"
    );
    let (status, body) = call(
        state,
        "GET",
        &format!("/internal/licences/{lid}/members"),
        &token(&keys, owner, &clock),
        None,
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("not-a-team"))
    );
    world.clean().await;
}

#[tokio::test]
async fn a_team_follows_its_size_at_stripe() {
    let Some(mut world) = World::with(false, Some(2)).await else {
        return;
    };
    let now = OffsetDateTime::now_utc();
    let owner = world.person().await;
    let double = Double::default();
    let (lid, sub, mut snapshot) = bought(&mut world, &double, owner, 3, now).await;
    assert_eq!(size_of(&mut world, &lid).await, (3, 6));
    snapshot.quantity = 2;
    double.set(snapshot);
    let event = world
        .event("customer.subscription.updated", json!({"id": sub}), now)
        .await;
    world.process(&double, event, now).await.unwrap();
    assert_eq!(size_of(&mut world, &lid).await, (2, 4));
    world.clean().await;
}

// ---- purchases without an account ----

/// What the stand-in of Supabase Auth does with the next invitation.
#[derive(Debug, Clone, Copy)]
enum Invite {
    /// Makes the user and answers it, as Auth does.
    Make,
    /// Answers 503 and makes nothing.
    Down,
    /// Makes the user, then fails: the answer is lost.
    MakeThenFail,
}

/// Supabase Auth's `POST /auth/v1/invite`, stood in on a loopback port: it
/// makes the user in the test database (the trigger makes `app.users`) and
/// records what it was asked.
#[derive(Debug, Default)]
struct GoTrue {
    db: String,
    next: Mutex<VecDeque<Invite>>,
    asked: Mutex<Vec<Value>>,
    made: Mutex<Vec<Uuid>>,
}

async fn gotrue(db: &str) -> (Accounts, Arc<GoTrue>) {
    let state = Arc::new(GoTrue {
        db: db.to_owned(),
        ..GoTrue::default()
    });
    let app = Router::new()
        .route(
            "/auth/v1/invite",
            post(
                |State(s): State<Arc<GoTrue>>,
                 Query(query): Query<HashMap<String, String>>,
                 headers: HeaderMap,
                 Json(body): Json<Value>| async move {
                    let header = |name: &str| {
                        headers
                            .get(name)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_owned()
                    };
                    s.asked.lock().unwrap().push(json!({
                        "body": body, "redirect_to": query.get("redirect_to"),
                        "apikey": header("apikey"), "authorization": header("authorization"),
                    }));
                    let next = s.next.lock().unwrap().pop_front().unwrap_or(Invite::Make);
                    if matches!(next, Invite::Down) {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"code": 503, "error_code": "unexpected_failure"})),
                        );
                    }
                    let mut conn = PgConnection::connect(&s.db).await.unwrap();
                    let email = body["email"].as_str().unwrap().to_owned();
                    let id = support::insert_auth_user(&mut conn, &email).await;
                    s.made.lock().unwrap().push(id);
                    if matches!(next, Invite::MakeThenFail) {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"code": 500, "error_code": "unexpected_failure"})),
                        );
                    }
                    (StatusCode::OK, Json(json!({"id": id, "email": email})))
                },
            ),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let accounts = Accounts::with_key(
        &format!("http://{addr}/auth/v1"),
        SecretKey::parse("sb_secret_purchasetests").unwrap(),
        "https://web.example.invalid",
    )
    .unwrap();
    (accounts, state)
}

impl World {
    async fn checkout_row(&mut self, id: Uuid) -> sqlx::postgres::PgRow {
        sqlx::query("select user_id, buyer, status, licence_id from app.checkouts where id = $1")
            .bind(id)
            .fetch_one(&mut self.owner)
            .await
            .unwrap()
    }

    async fn mail_jobs(&mut self, licence: Uuid) -> Vec<Value> {
        sqlx::query_scalar(
            "select payload from app.jobs where kind = 'mail_licence'
             and payload->>'licence' = $1::text and done_at is null",
        )
        .bind(licence)
        .fetch_all(&mut self.owner)
        .await
        .unwrap()
    }

    /// Takes the users the stand-in made, so that `clean` deletes them.
    fn adopt(&mut self, auth: &GoTrue) {
        for id in auth.made.lock().unwrap().iter() {
            if !self.people.contains(id) {
                self.people.push(*id);
            }
        }
    }
}

#[tokio::test]
async fn a_guest_purchase_invites_its_new_buyer_once_and_the_licence_is_theirs() {
    let Some(mut world) = World::new(false).await else {
        return;
    };
    let now = OffsetDateTime::now_utc();
    let (accounts, auth) = gotrue(&world.url).await;
    let (checkout, session) = world.guest_checkout("ru", now).await;
    let typed = World::new_email();
    let (sub, cus) = ids();
    let double = Double::default();
    double
        .emails
        .lock()
        .unwrap()
        .insert(session.clone(), typed.clone());
    let mut snapshot = world.snapshot(&sub, &cus, now + Duration::days(30));
    snapshot.checkout = Some(checkout);
    double.set(snapshot);

    // The completed checkout and its paid invoice, at once.
    let completed = world
        .event(
            "checkout.session.completed",
            session_of(checkout, &session, &cus, &sub),
            now,
        )
        .await;
    let paid = world.event("invoice.paid", paid_invoice(&sub), now).await;
    let (a, b) = tokio::join!(
        world.process_with(&double, Some(&accounts), completed, now),
        world.process_with(&double, Some(&accounts), paid, now)
    );
    a.unwrap();
    b.unwrap();
    world.adopt(&auth);

    let asked = auth.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "invited once: {asked:?}");
    let email = typed.trim().to_lowercase();
    assert_eq!(
        asked[0]["body"]["email"],
        email.as_str(),
        "trimmed, lower case, nothing else"
    );
    assert_eq!(
        asked[0]["body"]["data"],
        json!({"locale": "ru", "app": "devtab", "source": "purchase",
               "checkout": checkout.to_string(), "team": false})
    );
    assert_eq!(
        asked[0]["redirect_to"],
        "https://web.example.invalid/auth/callback/"
    );
    assert_eq!(asked[0]["apikey"], "sb_secret_purchasetests");
    assert_eq!(asked[0]["authorization"], "Bearer sb_secret_purchasetests");
    let buyer = auth.made.lock().unwrap()[0];

    let licences = world.licences_of(&sub).await;
    assert_eq!(licences.len(), 1);
    let licence: Uuid = licences[0].get("id");
    assert_eq!(licences[0].get::<Option<Uuid>, _>("user_id"), Some(buyer));
    let row = world.checkout_row(checkout).await;
    assert_eq!(row.get::<Option<Uuid>, _>("user_id"), Some(buyer));
    assert_eq!(
        row.get::<Option<String>, _>("buyer").as_deref(),
        Some("invited")
    );
    assert_eq!(row.get::<String, _>("status"), "completed");
    let customer: Option<Uuid> = sqlx::query_scalar(
        "select user_id from app.customers where provider = 'stripe' and provider_customer_id = $1",
    )
    .bind(&cus)
    .fetch_one(&mut world.owner)
    .await
    .unwrap();
    assert_eq!(customer, Some(buyer));
    assert!(
        world.mail_jobs(licence).await.is_empty(),
        "the invite is the mail"
    );

    // Later events of the subscription invite nobody.
    let event = world
        .event("customer.subscription.updated", json!({"id": sub}), now)
        .await;
    world
        .process_with(&double, Some(&accounts), event, now)
        .await
        .unwrap();
    assert_eq!(auth.asked.lock().unwrap().len(), 1);
    world.clean().await;
}

#[tokio::test]
async fn a_guest_buyer_who_has_an_account_is_not_invited_and_a_licence_mail_waits() {
    let Some(mut world) = World::new(false).await else {
        return;
    };
    let now = OffsetDateTime::now_utc();
    let (accounts, auth) = gotrue(&world.url).await;
    let email = format!("member-{}@example.invalid", Uuid::new_v4().simple());
    let person = support::insert_auth_user(&mut world.owner, &email).await;
    world.people.push(person);
    let double = Double::default();

    // A purchase signed in first: the person is a customer already.
    let signed_in = world.checkout(person, now).await;
    let (first_sub, first_cus) = ids();
    double.set(world.snapshot(&first_sub, &first_cus, now + Duration::days(30)));
    let event = world
        .event(
            "checkout.session.completed",
            session(signed_in, &first_cus, &first_sub),
            now,
        )
        .await;
    world.process(&double, event, now).await.unwrap();

    // Then without signing in, with the address in capitals: a new customer.
    let (checkout, session) = world.guest_checkout("en", now).await;
    double
        .emails
        .lock()
        .unwrap()
        .insert(session.clone(), format!("  {}  ", email.to_uppercase()));
    let (sub, cus) = ids();
    double.set(world.snapshot(&sub, &cus, now + Duration::days(30)));
    let event = world
        .event(
            "checkout.session.completed",
            session_of(checkout, &session, &cus, &sub),
            now,
        )
        .await;
    world
        .process_with(&double, Some(&accounts), event, now)
        .await
        .unwrap();

    assert!(auth.asked.lock().unwrap().is_empty(), "nobody invited");
    let licence: Uuid = world.licences_of(&sub).await[0].get("id");
    assert_eq!(
        world.licences_of(&sub).await[0].get::<Option<Uuid>, _>("user_id"),
        Some(person)
    );
    let row = world.checkout_row(checkout).await;
    assert_eq!(
        row.get::<Option<String>, _>("buyer").as_deref(),
        Some("existing")
    );
    let customers: i64 =
        sqlx::query_scalar("select count(*) from app.customers where user_id = $1")
            .bind(person)
            .fetch_one(&mut world.owner)
            .await
            .unwrap();
    assert_eq!(customers, 2, "a guest purchase brings its own customer");
    assert_eq!(
        world.mail_jobs(licence).await,
        vec![json!({"licence": licence, "checkout": checkout})]
    );
    // The worker does not run it yet: it waits for the mail adapter.
    let pool = support::api_pool(&world.url).await;
    let mut tx = pool.begin().await.unwrap();
    let due = jobs::claim_due(&mut *tx, now + Duration::days(1), 1000, &worker::RUNS)
        .await
        .unwrap();
    assert!(due.iter().all(|job| job.kind != worker::MAIL_LICENCE));
    tx.rollback().await.unwrap();
    world.clean().await;
}

#[tokio::test]
async fn a_failed_invite_is_tried_again_without_inviting_twice() {
    let Some(mut world) = World::new(false).await else {
        return;
    };
    let now = OffsetDateTime::now_utc();
    let (accounts, auth) = gotrue(&world.url).await;
    let (checkout, session) = world.guest_checkout("en", now).await;
    let double = Double::default();
    double
        .emails
        .lock()
        .unwrap()
        .insert(session.clone(), World::new_email());
    let (sub, cus) = ids();
    double.set(world.snapshot(&sub, &cus, now + Duration::days(30)));
    let event = world
        .event(
            "checkout.session.completed",
            session_of(checkout, &session, &cus, &sub),
            now,
        )
        .await;

    // No secret key: the checkout waits, nothing is asked.
    let err = world.process(&double, event, now).await.unwrap_err();
    assert!(
        matches!(&err, ProcessError::Retry(m) if m.contains("DEVTAB_SUPABASE_SECRET_KEY_FILE")),
        "{err}"
    );
    // Auth is down: tried again later.
    auth.next
        .lock()
        .unwrap()
        .extend([Invite::Down, Invite::MakeThenFail]);
    let err = world
        .process_with(&double, Some(&accounts), event, now)
        .await
        .unwrap_err();
    assert!(matches!(err, ProcessError::Retry(_)), "{err}");
    // Auth makes the user but the answer is lost: the transaction is undone.
    let err = world
        .process_with(&double, Some(&accounts), event, now)
        .await
        .unwrap_err();
    assert!(matches!(err, ProcessError::Retry(_)), "{err}");
    world.adopt(&auth);
    assert!(world.licences_of(&sub).await.is_empty());
    // The next attempt finds that user instead of inviting again.
    world
        .process_with(&double, Some(&accounts), event, now)
        .await
        .unwrap();
    assert_eq!(auth.asked.lock().unwrap().len(), 2, "no third invitation");
    let made = auth.made.lock().unwrap().clone();
    assert_eq!(made.len(), 1, "one user");
    assert_eq!(
        world.licences_of(&sub).await[0].get::<Option<Uuid>, _>("user_id"),
        Some(made[0])
    );
    world.clean().await;
}

#[tokio::test]
async fn a_team_bought_without_an_account_belongs_to_its_buyer() {
    let Some(mut world) = World::with(false, Some(2)).await else {
        return;
    };
    let clock = ManualClock::new(OffsetDateTime::now_utc());
    let now = clock.now();
    let keys = keys();
    let (accounts, auth) = gotrue(&world.url).await;
    let state = support::api_state(&world.url, shared(&clock), &Config::default())
        .await
        .with_auth(Verifier::fixed(
            &json!({"keys": [keys.jwk]}),
            ISSUER.to_owned(),
            shared(&clock),
        ));
    let (checkout, session) = world.guest_checkout("en", now).await;
    let double = Double::default();
    double
        .emails
        .lock()
        .unwrap()
        .insert(session.clone(), World::new_email());
    let (sub, cus) = ids();
    let mut snapshot = world.snapshot(&sub, &cus, now + Duration::days(30));
    snapshot.quantity = 3;
    double.set(snapshot);
    let event = world
        .event(
            "checkout.session.completed",
            session_of(checkout, &session, &cus, &sub),
            now,
        )
        .await;
    world
        .process_with(&double, Some(&accounts), event, now)
        .await
        .unwrap();
    world.adopt(&auth);
    assert_eq!(auth.asked.lock().unwrap()[0]["body"]["data"]["team"], true);
    let owner = auth.made.lock().unwrap()[0];
    let lid: String =
        sqlx::query_scalar("select lid from app.licences where provider_subscription_id = $1")
            .bind(&sub)
            .fetch_one(&mut world.owner)
            .await
            .unwrap();
    assert_eq!(size_of(&mut world, &lid).await, (3, 6));
    // The buyer owns the team once they open their account.
    let (status, team) = call(
        state,
        "GET",
        &format!("/internal/licences/{lid}/members"),
        &token(&keys, owner, &clock),
        None,
    )
    .await;
    assert_eq!(
        (status, &team["members"]),
        (StatusCode::OK, &json!(3)),
        "{team}"
    );
    world.clean().await;
}

#[tokio::test]
async fn a_guest_checkout_starts_without_an_account_and_its_browser_alone_sees_the_key() {
    let Some(mut world) = World::new(true).await else {
        return;
    };
    let clock = ManualClock::new(OffsetDateTime::now_utc());
    let (api, stand) = stand_in().await;
    let (accounts, auth) = gotrue(&world.url).await;
    let guest = Config {
        guest_checkout: true,
        ..Config::default()
    };
    let base = support::api_state(&world.url, shared(&clock), &guest).await;

    // Off, or on without the key to invite with: refused, clearly.
    let off = support::api_state(&world.url, shared(&clock), &Config::default())
        .await
        .with_payments(payments(&api))
        .with_accounts(accounts.clone());
    let body = json!({"plan": world.plan, "locale": "ru"});
    let (status, answer) = call(
        off,
        "POST",
        "/internal/checkouts/guest",
        "",
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        (status, answer["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("guest-checkout-off"))
    );
    let keyless = base.clone().with_payments(payments(&api));
    let (status, answer) = call(
        keyless,
        "POST",
        "/internal/checkouts/guest",
        "",
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        (status, answer["error"].as_str()),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("guest-checkout-unavailable")
        )
    );

    let state = base.with_payments(payments(&api)).with_accounts(accounts);
    let (status, started) = call(
        state.clone(),
        "POST",
        "/internal/checkouts/guest",
        "",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    let checkout = Uuid::parse_str(started["checkout"].as_str().unwrap()).unwrap();
    let claim = started["claim"].as_str().unwrap().to_owned();
    assert!(claim.len() >= 43);
    {
        let sent = stand.checkouts.lock().unwrap();
        let form = sent.last().unwrap();
        assert!(
            !form
                .iter()
                .any(|(key, _)| key == "customer" || key == "customer_email"),
            "the provider asks the buyer for the address: {form:?}"
        );
        assert!(form.contains(&(
            "success_url".to_owned(),
            format!("https://web.example.invalid/ru/pro/thanks/?ref={checkout}")
        )));
    }
    let row = world.checkout_row(checkout).await;
    assert_eq!(row.get::<Option<Uuid>, _>("user_id"), None);
    let stored: Vec<u8> = sqlx::query_scalar("select claim_hash from app.checkouts where id = $1")
        .bind(checkout)
        .fetch_one(&mut world.owner)
        .await
        .unwrap();
    assert_eq!(
        stored,
        devtab_api::licence::secret::hash(&claim),
        "only its hash"
    );

    let path = format!("/internal/checkouts/{checkout}/claim");
    let read = |claim: String| {
        let (state, path) = (state.clone(), path.clone());
        async move { call(state, "POST", &path, "", Some(json!({ "claim": claim }))).await }
    };
    let other = devtab_api::licence::secret::generate().unwrap().secret;
    assert_eq!(read(other).await.0, StatusCode::NOT_FOUND, "another secret");
    assert_eq!(read("short".to_owned()).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        read(claim.clone()).await,
        (
            StatusCode::OK,
            json!({"status": "open", "key": null, "key_hint": null, "account": null})
        )
    );

    // The payment arrives through the webhook and the worker.
    let (sub, cus) = ids();
    let now = clock.now();
    *stand.email.lock().unwrap() = Some(World::new_email());
    *stand.subscription.lock().unwrap() = Some(json!({
        "id": sub, "status": "active", "customer": cus, "cancel_at_period_end": false,
        "metadata": {"devtab_checkout": checkout.to_string()},
        "items": {"data": [{"price": {"id": world.price}, "current_period_end": (now + Duration::days(30)).unix_timestamp()}]},
        "latest_invoice": {"id": "in_1", "status": "paid"},
    }));
    let session_id: String =
        sqlx::query_scalar("select provider_checkout_id from app.checkouts where id = $1")
            .bind(checkout)
            .fetch_one(&mut world.owner)
            .await
            .unwrap();
    let event = json!({
        "id": format!("evt_{}", Uuid::new_v4().simple()), "object": "event",
        "type": "checkout.session.completed", "created": now.unix_timestamp(),
        "api_version": stripe::DEFAULT_API_VERSION, "livemode": false,
        "data": {"object": session_of(checkout, &session_id, &cus, &sub)},
    })
    .to_string();
    let (status, _) = deliver(state.clone(), "stripe", &event, &signed(&event, now)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(worker::run_due(&state).await.unwrap() >= 1);
    world.adopt(&auth);
    assert_eq!(auth.asked.lock().unwrap().len(), 1);

    let (status, seen) = read(claim.clone()).await;
    assert_eq!(
        (status, &seen["status"]),
        (StatusCode::OK, &json!("completed")),
        "{seen}"
    );
    assert_eq!(seen["account"], "invited");
    let key = seen["key"].as_str().expect("the key, the first time");
    assert!(key.starts_with("DTD-"));
    let (_, again) = read(claim).await;
    assert_eq!(again["key"], Value::Null, "only once");
    assert_eq!(again["key_hint"], seen["key_hint"]);
    world.clean().await;
}

/// A completed Checkout Session of ours whose id is `id`.
fn session_of(checkout: Uuid, id: &str, customer: &str, subscription: &str) -> Value {
    let mut object = session(checkout, customer, subscription);
    object["id"] = json!(id);
    object
}
