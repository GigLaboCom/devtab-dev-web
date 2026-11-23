//! Database tests against the local Supabase Postgres with the migrations of
//! `supabase/migrations/` applied, named by `DEVTAB_TEST_DATABASE_URL` (the
//! owner's URL, the `postgres` role that `supabase status` reports). Each test
//! runs inside a transaction that is rolled back. Without the variable the
//! tests are skipped, so `cargo test` works without a database.
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
async fn the_api_role_exists_and_can_log_in() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let (login, bypass_rls): (bool, bool) = sqlx::query_as(
        "select rolcanlogin, rolbypassrls from pg_roles where rolname = 'devtab_api'",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert!(login, "LOGIN");
    assert!(bypass_rls, "BYPASSRLS");
}

/// Each environment gives the API role its own password (locally
/// `script/db-up`), so the database cannot tell whether a migration set one:
/// the migrations themselves must not mention one outside their comments.
#[test]
fn the_migrations_set_no_password() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../supabase/migrations");
    let mut migrations = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "sql") {
            continue;
        }
        migrations += 1;
        let sql = std::fs::read_to_string(&path).unwrap();
        for (n, line) in sql.lines().enumerate() {
            let code = line.split("--").next().unwrap().to_ascii_lowercase();
            assert!(
                !code.contains("password"),
                "{}:{}: {line}",
                path.display(),
                n + 1
            );
        }
    }
    assert!(migrations > 0, "no migrations in {}", dir.display());
}

#[tokio::test]
async fn the_data_api_roles_get_nothing_in_schema_app() {
    let Some(mut conn) = connect().await else {
        return;
    };
    for (role, set_role) in [
        ("anon", "set local role anon"),
        ("authenticated", "set local role authenticated"),
        ("service_role", "set local role service_role"),
    ] {
        let (usage, create, execute): (bool, bool, bool) = sqlx::query_as(
            "select has_schema_privilege($1, 'app', 'USAGE'),
                    has_schema_privilege($1, 'app', 'CREATE'),
                    has_function_privilege($1, 'app.set_updated_at()', 'EXECUTE')",
        )
        .bind(role)
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert!(!usage, "{role}: no USAGE on app");
        assert!(!create, "{role}: no CREATE on app");
        assert!(!execute, "{role}: no EXECUTE on app.set_updated_at()");

        // What the Data API would do on the role's behalf.
        let mut tx = conn.begin().await.unwrap();
        sqlx::query("create table app.data_api_probe (id integer)")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(set_role).execute(&mut *tx).await.unwrap();
        let err = sqlx::query("select * from app.data_api_probe")
            .execute(&mut *tx)
            .await
            .unwrap_err();
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some(PERMISSION_DENIED),
            "{role}"
        );
        tx.rollback().await.unwrap();
    }
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
