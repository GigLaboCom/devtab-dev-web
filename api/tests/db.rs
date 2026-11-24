//! Database tests against the local Supabase Postgres with the migrations of
//! `supabase/migrations/` applied, named by `DEVTAB_TEST_DATABASE_URL` (the
//! owner's URL, the `postgres` role that `supabase status` reports). Each test
//! runs inside a transaction that is rolled back. Without the variable the
//! tests are skipped, so `cargo test` works without a database.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod support;

use devtab_api::licence::key;
use devtab_api::store::licences::NewLicence;
use devtab_api::store::{audit, customers, installations, jobs, licences, plans, users};
use serde_json::json;
use sqlx::{Connection as _, PgConnection};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

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

/// Every table the migrations create in `app`.
const TABLES: [&str; 10] = [
    "admin_audit",
    "customers",
    "installations",
    "jobs",
    "licences",
    "plan_prices",
    "plan_texts",
    "plans",
    "settings",
    "users",
];

/// The functions in `app` that only the database itself may run.
const PRIVATE_FUNCTIONS: [&str; 2] = ["app.provision_user(uuid)", "app.on_auth_user_created()"];
/// The functions the API role may run, and nobody else.
const API_FUNCTIONS: [&str; 2] = ["app.user_email(uuid)", "app.user_id_by_email(text)"];

fn code(err: &sqlx::Error) -> Option<String> {
    err.as_database_error()
        .and_then(|e| e.code().map(std::borrow::Cow::into_owned))
}

#[tokio::test]
async fn row_level_security_is_on_for_every_table_in_app() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let tables: Vec<(String, bool)> = sqlx::query_as(
        "select c.relname::text, c.relrowsecurity from pg_class c
         join pg_namespace n on n.oid = c.relnamespace
         where n.nspname = 'app' and c.relkind in ('r', 'p')
         order by 1",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    let names: Vec<&str> = tables.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, TABLES);
    for (name, rls) in &tables {
        assert!(rls, "row level security on app.{name}");
    }
}

#[tokio::test]
async fn the_api_role_uses_every_table_reads_no_auth_and_only_appends_to_the_audit() {
    let Some(mut conn) = connect().await else {
        return;
    };
    for table in TABLES {
        let (select, insert, update, delete): (bool, bool, bool, bool) = sqlx::query_as(
            "select has_table_privilege('devtab_api', $1, 'SELECT'),
                    has_table_privilege('devtab_api', $1, 'INSERT'),
                    has_table_privilege('devtab_api', $1, 'UPDATE'),
                    has_table_privilege('devtab_api', $1, 'DELETE')",
        )
        .bind(format!("app.{table}"))
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert!(select && insert, "{table}: select and insert");
        let changes = table != "admin_audit";
        assert_eq!(
            (update, delete),
            (changes, changes),
            "{table}: update and delete"
        );
    }
    for function in API_FUNCTIONS {
        let (api,): (bool,) =
            sqlx::query_as("select has_function_privilege('devtab_api', $1, 'EXECUTE')")
                .bind(function)
                .fetch_one(&mut conn)
                .await
                .unwrap();
        assert!(api, "devtab_api may run {function}");
    }

    let mut tx = conn.begin().await.unwrap();
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = sqlx::query("select 1 from auth.users limit 1")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(code(&err).as_deref(), Some(PERMISSION_DENIED), "auth.users");
    tx.rollback().await.unwrap();

    let mut tx = conn.begin().await.unwrap();
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    let id = audit::record(
        &mut *tx,
        "test",
        "test.append",
        None,
        &json!({}),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    for sql in [
        "update app.admin_audit set action = 'changed' where id = $1",
        "delete from app.admin_audit where id = $1",
    ] {
        sqlx::query("savepoint probe")
            .execute(&mut *tx)
            .await
            .unwrap();
        let err = sqlx::query(sql)
            .bind(id)
            .execute(&mut *tx)
            .await
            .unwrap_err();
        assert_eq!(code(&err).as_deref(), Some(PERMISSION_DENIED), "{sql}");
        sqlx::query("rollback to savepoint probe")
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_data_api_roles_get_nothing_in_any_table_or_function() {
    let Some(mut conn) = connect().await else {
        return;
    };
    for role in ["anon", "authenticated", "service_role"] {
        for table in TABLES {
            let (any,): (bool,) = sqlx::query_as(
                "select has_table_privilege($1, $2, 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')",
            )
            .bind(role)
            .bind(format!("app.{table}"))
            .fetch_one(&mut conn)
            .await
            .unwrap();
            assert!(!any, "{role}: nothing on app.{table}");
        }
        for function in PRIVATE_FUNCTIONS.iter().chain(&API_FUNCTIONS) {
            let (execute,): (bool,) =
                sqlx::query_as("select has_function_privilege($1, $2, 'EXECUTE')")
                    .bind(role)
                    .bind(function)
                    .fetch_one(&mut conn)
                    .await
                    .unwrap();
            assert!(!execute, "{role}: no EXECUTE on {function}");
        }
        for sql in [
            "select * from app.users",
            "select * from app.licences",
            "select app.user_email(gen_random_uuid())",
            "select app.user_id_by_email('a@example.invalid')",
            "select app.provision_user(gen_random_uuid())",
        ] {
            let mut tx = conn.begin().await.unwrap();
            sqlx::query(sqlx::AssertSqlSafe(format!("set local role {role}")))
                .execute(&mut *tx)
                .await
                .unwrap();
            let err = sqlx::query(sql).execute(&mut *tx).await.unwrap_err();
            assert_eq!(
                code(&err).as_deref(),
                Some(PERMISSION_DENIED),
                "{role}: {sql}"
            );
            tx.rollback().await.unwrap();
        }
    }
    for function in PRIVATE_FUNCTIONS {
        let (api,): (bool,) =
            sqlx::query_as("select has_function_privilege('devtab_api', $1, 'EXECUTE')")
                .bind(function)
                .fetch_one(&mut conn)
                .await
                .unwrap();
        assert!(!api, "devtab_api may not run {function}");
    }
}

#[tokio::test]
async fn a_new_auth_user_gets_a_user_row_and_provisioning_again_changes_nothing() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let mut tx = conn.begin().await.unwrap();
    let id = support::insert_auth_user(&mut tx, "provision@example.invalid").await;
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    let user = users::find(&mut *tx, id).await.unwrap().unwrap();
    assert_eq!(user.status, "active");
    sqlx::query("reset role").execute(&mut *tx).await.unwrap();
    sqlx::query("select app.provision_user($1)")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let (rows,): (i64,) = sqlx::query_as("select count(*) from app.users where id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows, 1);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_api_role_finds_users_by_email_and_their_email() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let mut tx = conn.begin().await.unwrap();
    let id = support::insert_auth_user(&mut tx, "lookup@example.invalid").await;
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        users::id_by_email(&mut *tx, "  Lookup@Example.Invalid ")
            .await
            .unwrap(),
        Some(id)
    );
    assert_eq!(
        users::id_by_email(&mut *tx, "nobody@example.invalid")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        users::email_of(&mut *tx, id).await.unwrap().as_deref(),
        Some("lookup@example.invalid")
    );
    assert!(users::set_status(&mut *tx, id, "blocked").await.unwrap());
    assert_eq!(
        users::find(&mut *tx, id).await.unwrap().unwrap().status,
        "blocked"
    );
    assert!(
        !users::set_status(&mut *tx, Uuid::new_v4(), "blocked")
            .await
            .unwrap()
    );
    tx.rollback().await.unwrap();
}

async fn subscription(tx: &mut PgConnection, user: Uuid, customer: Uuid) -> Uuid {
    let new_key = key::generate().unwrap();
    let lid = key::new_lid().unwrap();
    licences::insert(
        tx,
        &NewLicence {
            lid: &lid,
            user_id: Some(user),
            customer_id: Some(customer),
            plan_id: "test-monthly",
            kind: "subscription",
            seats: 2,
            paid_until: Some(OffsetDateTime::now_utc() + Duration::days(30)),
            cancel_at_period_end: false,
            provider: Some("test"),
            provider_subscription_id: Some(&format!("sub_TEST_{}", Uuid::new_v4())),
            key_hash: &new_key.hash,
            key_hint: &new_key.hint,
            note: None,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn deleting_an_auth_user_removes_the_user_row_and_detaches_licences_and_customers() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let mut tx = conn.begin().await.unwrap();
    support::ensure_test_plans(&mut tx).await;
    let user = support::insert_auth_user(&mut tx, "cascade@example.invalid").await;
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    let customer = customers::insert(&mut *tx, user, "test", "cus_TEST_cascade")
        .await
        .unwrap();
    let licence = subscription(&mut tx, user, customer).await;
    sqlx::query("reset role").execute(&mut *tx).await.unwrap();

    sqlx::query("delete from auth.users where id = $1")
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(users::find(&mut *tx, user).await.unwrap(), None);
    let (licence_user,): (Option<Uuid>,) =
        sqlx::query_as("select user_id from app.licences where id = $1")
            .bind(licence)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(licence_user, None, "the licence stays, without its owner");
    let row = customers::find(&mut *tx, customer).await.unwrap().unwrap();
    assert_eq!(row.user_id, None, "the customer stays, without its user");
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_store_queries_work_as_the_api_role() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let mut tx = conn.begin().await.unwrap();
    support::ensure_test_plans(&mut tx).await;
    let user = support::insert_auth_user(&mut tx, "store@example.invalid").await;
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    let now = OffsetDateTime::now_utc();

    let plan = plans::find(&mut *tx, "test-monthly")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (plan.kind.as_str(), plan.interval.as_deref(), plan.seats),
        ("subscription", Some("month"), Some(2))
    );

    let customer = customers::insert(&mut *tx, user, "test", "cus_TEST_store")
        .await
        .unwrap();
    let id = subscription(&mut tx, user, customer).await;
    let row = licences::of_user(&mut *tx, user)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        (row.id, row.term.as_deref(), row.key_version),
        (id, Some("month"), 1)
    );
    let found = licences::find_by_lid(&mut *tx, &row.lid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found, row);

    let replacement = key::generate().unwrap();
    let version = licences::replace_key(&mut *tx, id, &replacement.hash, &replacement.hint)
        .await
        .unwrap();
    assert_eq!(version, 2);
    let by_key = licences::find_by_key_hash_for_update(&mut *tx, &replacement.hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_key.key_hint, replacement.hint);

    let secret = [7_u8; 32];
    installations::activate(&mut *tx, id, "q3V2mZ0bTEST0000000000", &secret, now)
        .await
        .unwrap();
    assert_eq!(installations::count_active(&mut *tx, id).await.unwrap(), 1);
    assert_eq!(
        installations::count_activated_since(&mut *tx, id, now - Duration::days(30))
            .await
            .unwrap(),
        1
    );

    licences::block_key(&mut *tx, id, now).await.unwrap();
    let blocked = licences::find_by_lid(&mut *tx, &row.lid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (blocked.status.as_str(), blocked.revoke_reason.as_deref()),
        ("revoked", Some("key-blocked"))
    );
    assert!(blocked.key_blocked_at.is_some());

    let job = jobs::enqueue(&mut *tx, "test", &json!({ "licence": id }), now)
        .await
        .unwrap();
    let due = jobs::claim_due(&mut *tx, now, 100).await.unwrap();
    assert!(due.iter().any(|j| j.id == job));
    jobs::fail(&mut *tx, job, "test failure", now + Duration::minutes(5))
        .await
        .unwrap();
    assert!(
        !jobs::claim_due(&mut *tx, now, 100)
            .await
            .unwrap()
            .iter()
            .any(|j| j.id == job)
    );
    jobs::complete(&mut *tx, job, now).await.unwrap();
    assert!(
        !jobs::claim_due(&mut *tx, now + Duration::hours(1), 100)
            .await
            .unwrap()
            .iter()
            .any(|j| j.id == job)
    );

    let entry = audit::record(
        &mut *tx,
        "test",
        "test.store",
        Some(id),
        &json!({ "k": 1 }),
        now,
    )
    .await
    .unwrap();
    let recent = audit::of_licence(&mut *tx, id, 5).await.unwrap();
    assert_eq!(recent[0].id, entry);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_tables_refuse_inconsistent_rows() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let mut tx = conn.begin().await.unwrap();
    support::ensure_test_plans(&mut tx).await;
    let hash = "decode(repeat('ab', 32), 'hex')";
    for sql in [
        "insert into app.plans (id, kind, sort) values ('test-x', 'subscription', 1)".to_owned(),
        "insert into app.plans (id, kind, \"interval\", sort) values ('test-y', 'subscription', 'month', 1)".to_owned(),
        "insert into app.plans (id, kind, sort) values ('Test Z', 'free', 1)".to_owned(),
        format!(
            "insert into app.licences (lid, plan_id, kind, seats, key_hash, key_hint)
             values ('lic_00000000000000000000000001', 'test-monthly', 'subscription', 2, {hash}, 'ABCD')"
        ),
        format!(
            "insert into app.licences (lid, plan_id, kind, status, seats, key_hash, key_hint)
             values ('lic_00000000000000000000000002', 'test-monthly', 'complimentary', 'revoked', 2, {hash}, 'ABCD')"
        ),
        format!(
            "insert into app.licences (lid, plan_id, kind, seats, key_hash, key_hint)
             values ('lic_short', 'test-monthly', 'complimentary', 2, {hash}, 'ABCD')"
        ),
        "insert into app.licences (lid, plan_id, kind, seats, key_hash, key_hint)
         values ('lic_00000000000000000000000003', 'test-monthly', 'complimentary', 2, decode('ab', 'hex'), 'ABCD')"
            .to_owned(),
    ] {
        sqlx::query("savepoint probe").execute(&mut *tx).await.unwrap();
        let err = sqlx::query(sqlx::AssertSqlSafe(sql.clone()))
            .execute(&mut *tx)
            .await
            .unwrap_err();
        assert_eq!(code(&err).as_deref(), Some("23514"), "{sql}");
        sqlx::query("rollback to savepoint probe").execute(&mut *tx).await.unwrap();
    }
    tx.rollback().await.unwrap();
}
