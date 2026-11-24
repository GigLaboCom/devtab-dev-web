//! Shared by the integration tests: paths, a test signing key made in memory
//! for this test process (never read from the repository), the database
//! (`DEVTAB_TEST_DATABASE_URL`) and a validator for the contract's JSON
//! schemas.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

pub mod schema;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use devtab_api::clock::SharedClock;
use devtab_api::licence::signer::{self, KeyList, PublishedKey, Signer};
use devtab_api::{AppState, Config, Store};
use p256::ecdsa::VerifyingKey;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection as _, Executor as _, PgConnection, PgPool};

/// A path from the repository's root.
pub fn repo(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(path)
}

/// This process's test key: its PEM and public entry, made once in memory.
fn test_key() -> &'static (String, PublishedKey) {
    static KEY: OnceLock<(String, PublishedKey)> = OnceLock::new();
    KEY.get_or_init(|| {
        let start = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        signer::generate(None, true, start).unwrap()
    })
}

/// A signer with this process's test key.
pub fn test_signer() -> Signer {
    let (pem, entry) = test_key();
    Signer::from_pkcs8_pem(&entry.kid, pem).unwrap()
}

/// The key list holding this process's test key, as a developer's
/// `DEVTAB_DEV_SIGNING_KEYS_FILE` would.
pub fn test_key_list() -> KeyList {
    KeyList {
        note: None,
        keys: vec![test_key().1.clone()],
    }
}

/// What tokens signed by [`test_signer`] are checked against.
pub fn test_verifiers() -> Vec<(String, VerifyingKey)> {
    test_key_list().verifiers()
}

/// The owner's URL of the test database, or `None` (and a note) without one.
pub fn database_url() -> Option<String> {
    let url = std::env::var("DEVTAB_TEST_DATABASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty());
    if url.is_none() {
        eprintln!("DEVTAB_TEST_DATABASE_URL is not set, skipping the database test");
    }
    url
}

pub async fn connect() -> Option<PgConnection> {
    let url = database_url()?;
    Some(PgConnection::connect(&url).await.unwrap())
}

/// A pool whose connections act as the API role (`set role devtab_api`), as
/// the API itself connects: the owner's URL, the role's privileges.
pub async fn api_pool(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                conn.execute("set role devtab_api; set search_path = app")
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .unwrap()
}

/// The API's state over the test database, with the test key and `clock`.
pub async fn api_state(url: &str, clock: SharedClock, config: &Config) -> AppState {
    AppState::new(
        Store::from_pool(api_pool(url).await),
        clock,
        test_signer(),
        config,
    )
}

/// A state whose database is not there: fine for every route that does not
/// reach it.
pub fn offline_state(config: &Config) -> AppState {
    AppState::new(
        Store::connect_lazy("postgres://nobody@127.0.0.1:1/none", 1).unwrap(),
        Arc::new(devtab_api::clock::SystemClock),
        test_signer(),
        config,
    )
}

/// Plans the tests issue licences on, committed once and shared.
pub async fn ensure_test_plans(conn: &mut PgConnection) {
    sqlx::query(
        r#"insert into app.plans (id, kind, "interval", seats, sort, available) values
             ('test-monthly', 'subscription', 'month', 2, 90, false),
             ('test-annual', 'subscription', 'year', 2, 91, false),
             ('test-single', 'subscription', 'month', 1, 92, false)
           on conflict (id) do nothing"#,
    )
    .execute(conn)
    .await
    .unwrap();
}

/// A user of Supabase Auth, as Auth would insert one; the trigger makes the
/// `app.users` row.
pub async fn insert_auth_user(conn: &mut PgConnection, email: &str) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "insert into auth.users (id, instance_id, aud, role, email)
         values ($1, '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', $2)",
    )
    .bind(id)
    .bind(email)
    .execute(conn)
    .await
    .unwrap();
    id
}
