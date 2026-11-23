//! Database tests against a real Postgres with the migrations of
//! `db/migrations/` applied, named by `DEVTAB_TEST_DATABASE_URL` (a superuser
//! URL, such as the `postgres` role of `script/db-up`). Each test runs inside a
//! transaction that is rolled back. Without the variable the tests are skipped,
//! so `cargo test` works without a database.
#![allow(clippy::unwrap_used)]

use sqlx::{Connection as _, PgConnection};

/// `insufficient_privilege`.
const PERMISSION_DENIED: &str = "42501";

async fn connect() -> Option<PgConnection> {
    let Some(url) = std::env::var("DEVTAB_TEST_DATABASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        eprintln!("DEVTAB_TEST_DATABASE_URL is not set, skipping the database test");
        return None;
    };
    Some(PgConnection::connect(&url).await.unwrap())
}

#[tokio::test]
async fn schema_app_exists() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let (exists,): (bool,) =
        sqlx::query_as("select exists (select 1 from pg_namespace where nspname = 'app')")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert!(exists);
}

#[tokio::test]
async fn the_api_role_exists_and_can_log_in_without_a_password() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let (login, bypass_rls, password): (bool, bool, bool) = sqlx::query_as(
        "select rolcanlogin, rolbypassrls, rolpassword is not null
         from pg_authid where rolname = 'devtab_api'",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert!(login, "LOGIN");
    assert!(bypass_rls, "BYPASSRLS");
    assert!(!password, "the migration sets no password");
}

#[tokio::test]
async fn the_api_role_uses_schema_app_and_cannot_create_in_it() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let mut tx = conn.begin().await.unwrap();
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();

    let (usage, create): (bool, bool) = sqlx::query_as(
        "select has_schema_privilege('app', 'USAGE'), has_schema_privilege('app', 'CREATE')",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(usage, "USAGE on app");
    assert!(!create, "no CREATE on app");

    let err = sqlx::query("create table app.not_allowed (id integer)")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some(PERMISSION_DENIED)
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn set_updated_at_exists_and_only_the_api_role_may_run_it() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let (exists, api, public): (bool, bool, bool) = sqlx::query_as(
        "select to_regprocedure('app.set_updated_at()') is not null,
                has_function_privilege('devtab_api', 'app.set_updated_at()', 'EXECUTE'),
                exists (
                  select 1
                  from pg_proc p, aclexplode(coalesce(p.proacl, acldefault('f', p.proowner))) a
                  where p.oid = 'app.set_updated_at()'::regprocedure
                    and a.grantee = 0 and a.privilege_type = 'EXECUTE'
                )",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert!(exists, "app.set_updated_at()");
    assert!(api, "devtab_api may execute it");
    assert!(!public, "public may not execute it");
}
