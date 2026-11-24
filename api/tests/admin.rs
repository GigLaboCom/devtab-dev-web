//! The admin commands against the local Supabase database
//! (`DEVTAB_TEST_DATABASE_URL`), parsed as the command line would and run as
//! the API role in a transaction that is rolled back. Each change must leave
//! one `app.admin_audit` row.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod support;

use clap::Parser;
use devtab_api::admin::{self, AdminCommand, Context};
use devtab_api::licence::key;
use devtab_api::store::{installations, licences};
use serde_json::Value;
use sqlx::{Connection as _, PgConnection};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: AdminCommand,
}

fn parse(args: &[&str]) -> AdminCommand {
    Cli::try_parse_from(std::iter::once("admin").chain(args.iter().copied()))
        .unwrap()
        .command
}

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp()).unwrap()
}

async fn run_with(tx: &mut PgConnection, args: &[&str], confirm: bool) -> anyhow::Result<String> {
    let mut out = Vec::new();
    let mut ctx = Context {
        actor: "tester".to_owned(),
        now: now(),
        out: &mut out,
    };
    admin::execute(&parse(args), tx, &mut ctx, &mut || confirm).await?;
    Ok(String::from_utf8(out).unwrap())
}

async fn run(tx: &mut PgConnection, args: &[&str]) -> String {
    run_with(tx, args, true).await.unwrap()
}

/// The printed value after `label` ("lid  lic_…", "key  DTD-…").
fn printed<'a>(output: &'a str, label: &str) -> &'a str {
    output
        .lines()
        .find_map(|line| line.strip_prefix(label))
        .unwrap()
        .trim()
}

async fn audit_actions(tx: &mut PgConnection) -> Vec<(String, String, Option<Uuid>, Value)> {
    sqlx::query_as(
        "select actor, action, licence_id, details from app.admin_audit
         where actor = 'tester' order by id",
    )
    .fetch_all(tx)
    .await
    .unwrap()
}

/// A transaction acting as the API role, with the test plans and a user.
async fn begin<'c>(
    conn: &'c mut PgConnection,
    email: &str,
) -> (sqlx::Transaction<'c, sqlx::Postgres>, Uuid) {
    let mut tx = conn.begin().await.unwrap();
    support::ensure_test_plans(&mut tx).await;
    let user = support::insert_auth_user(&mut tx, email).await;
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    (tx, user)
}

#[tokio::test]
async fn comp_issues_a_licence_prints_its_key_once_and_records_it() {
    let Some(mut conn) = support::connect().await else {
        return;
    };
    let (mut tx, user) = begin(&mut conn, "comp@example.invalid").await;

    let output = run(&mut tx, &["licence", "comp", "--plan", "test-monthly"]).await;
    let lid = printed(&output, "lid ").to_owned();
    let shown = printed(&output, "key ").to_owned();
    let licence = licences::find_by_lid(&mut *tx, &lid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            licence.kind.as_str(),
            licence.status.as_str(),
            licence.seats,
            licence.user_id,
            licence.paid_until
        ),
        ("complimentary", "active", 2, None, None)
    );
    let hash = key::hash(&key::canonical(&shown).unwrap());
    let by_key = licences::find_by_key_hash_for_update(&mut *tx, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        by_key.id, licence.id,
        "only the hash of the printed key is stored"
    );
    assert!(shown.ends_with(&licence.key_hint));

    let output = run(
        &mut tx,
        &[
            "licence",
            "comp",
            "--plan",
            "test-annual",
            "--user",
            "Comp@Example.Invalid",
            "--until",
            "2099-01-01",
            "--note",
            "store reviewer",
        ],
    )
    .await;
    let second = licences::find_by_lid(&mut *tx, printed(&output, "lid "))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.user_id, Some(user));
    assert_eq!(
        second.paid_until.unwrap().unix_timestamp(),
        4_070_908_800,
        "2099-01-01T00:00:00Z"
    );
    assert_eq!(second.note.as_deref(), Some("store reviewer"));

    let audit = audit_actions(&mut tx).await;
    assert_eq!(audit.len(), 2);
    assert_eq!(
        (audit[0].1.as_str(), audit[0].2),
        ("licence.comp", Some(licence.id))
    );
    assert_eq!(audit[1].3["user"], user.to_string());
    assert_eq!(
        audit[1].3["note"], true,
        "the note itself stays out of the audit"
    );
    assert!(
        !audit.iter().any(|a| a.3.to_string().contains(&shown)),
        "no key in the audit"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn comp_refuses_what_it_cannot_issue() {
    let Some(mut conn) = support::connect().await else {
        return;
    };
    let (mut tx, _) = begin(&mut conn, "refuse@example.invalid").await;
    sqlx::query("reset role").execute(&mut *tx).await.unwrap();
    sqlx::query("insert into app.plans (id, kind, sort) values ('test-free', 'free', 99) on conflict do nothing")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    let yesterday = (OffsetDateTime::now_utc() - Duration::days(1)).date();
    let past = format!(
        "{:04}-{:02}-{:02}",
        yesterday.year(),
        u8::from(yesterday.month()),
        yesterday.day()
    );
    let long_note = "n".repeat(501);
    for args in [
        vec!["licence", "comp", "--plan", "no-such-plan"],
        vec!["licence", "comp", "--plan", "test-free"],
        vec![
            "licence",
            "comp",
            "--plan",
            "test-monthly",
            "--user",
            "nobody@example.invalid",
        ],
        vec![
            "licence",
            "comp",
            "--plan",
            "test-monthly",
            "--until",
            past.as_str(),
        ],
        vec![
            "licence",
            "comp",
            "--plan",
            "test-monthly",
            "--until",
            "someday",
        ],
        vec![
            "licence",
            "comp",
            "--plan",
            "test-monthly",
            "--note",
            long_note.as_str(),
        ],
    ] {
        assert!(run_with(&mut tx, &args, true).await.is_err(), "{args:?}");
    }
    assert!(audit_actions(&mut tx).await.is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn show_block_revoke_and_rotate_a_licence() {
    let Some(mut conn) = support::connect().await else {
        return;
    };
    let (mut tx, _) = begin(&mut conn, "show@example.invalid").await;
    let output = run(
        &mut tx,
        &[
            "licence",
            "comp",
            "--plan",
            "test-monthly",
            "--user",
            "show@example.invalid",
        ],
    )
    .await;
    let lid = printed(&output, "lid ").to_owned();
    let first_key = printed(&output, "key ").to_owned();
    let licence = licences::find_by_lid(&mut *tx, &lid)
        .await
        .unwrap()
        .unwrap();
    installations::activate(
        &mut *tx,
        licence.id,
        "q3V2mZ0bTEST0000000000",
        &[1; 32],
        now(),
    )
    .await
    .unwrap();

    let shown = run(&mut tx, &["licence", "show", &lid]).await;
    assert!(shown.contains(&format!("licence {lid}")));
    assert!(shown.contains("test-monthly (complimentary)"));
    assert!(shown.contains("2 (1 active)"));
    assert!(shown.contains("q3V2…  active"));
    assert!(
        !shown.contains("q3V2mZ0bTEST"),
        "only the first four characters of an installation"
    );
    assert!(shown.contains("licence.comp"));
    assert!(!shown.contains(&first_key), "never the key");
    let by_email = run(&mut tx, &["licence", "show", "show@example.invalid"]).await;
    assert!(by_email.contains(&format!("licence {lid}")));
    assert!(
        run_with(&mut tx, &["licence", "show", "somebody"], true)
            .await
            .is_err()
    );

    let rotated = run(&mut tx, &["licence", "rotate-key", &lid]).await;
    let second_key = printed(&rotated, "key ").to_owned();
    assert_ne!(second_key, first_key);
    let after = licences::find_by_lid(&mut *tx, &lid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.key_version, 2);
    let old_hash = key::hash(&key::canonical(&first_key).unwrap());
    assert!(
        licences::find_by_key_hash_for_update(&mut *tx, &old_hash)
            .await
            .unwrap()
            .is_none()
    );

    run(
        &mut tx,
        &["licence", "block", &lid, "--reason", "shared publicly"],
    )
    .await;
    let blocked = licences::find_by_lid(&mut *tx, &lid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (blocked.status.as_str(), blocked.revoke_reason.as_deref()),
        ("revoked", Some("key-blocked"))
    );
    assert!(
        run_with(&mut tx, &["licence", "rotate-key", &lid], true)
            .await
            .is_err()
    );

    let output = run(&mut tx, &["licence", "comp", "--plan", "test-monthly"]).await;
    let refunded = printed(&output, "lid ").to_owned();
    run(&mut tx, &["licence", "revoke", &refunded, "--refund"]).await;
    let row = licences::find_by_lid(&mut *tx, &refunded)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.status.as_str(), row.revoke_reason.as_deref()),
        ("revoked", Some("refund"))
    );
    assert!(
        Cli::try_parse_from(["admin", "licence", "revoke", &refunded]).is_err(),
        "--refund is required"
    );
    assert!(
        run_with(
            &mut tx,
            &[
                "licence",
                "block",
                "lic_00000000000000000000000000",
                "--reason",
                "x"
            ],
            true
        )
        .await
        .is_err()
    );

    let actions: Vec<String> = audit_actions(&mut tx)
        .await
        .into_iter()
        .map(|a| a.1)
        .collect();
    assert_eq!(
        actions,
        [
            "licence.comp",
            "licence.rotate-key",
            "licence.block",
            "licence.comp",
            "licence.revoke"
        ]
    );
    let audit = audit_actions(&mut tx).await;
    assert_eq!(audit[2].3["reason"], "shared publicly");
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn users_are_blocked_and_unblocked_and_installations_released() {
    let Some(mut conn) = support::connect().await else {
        return;
    };
    let (mut tx, user) = begin(&mut conn, "block@example.invalid").await;
    run(&mut tx, &["user", "block", "block@example.invalid"]).await;
    let (status,): (String,) = sqlx::query_as("select status from app.users where id = $1")
        .bind(user)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(status, "blocked");
    run(&mut tx, &["user", "unblock", "block@example.invalid"]).await;
    assert!(
        run_with(&mut tx, &["user", "block", "nobody@example.invalid"], true)
            .await
            .is_err()
    );

    let output = run(&mut tx, &["licence", "comp", "--plan", "test-monthly"]).await;
    let licence = licences::find_by_lid(&mut *tx, printed(&output, "lid "))
        .await
        .unwrap()
        .unwrap();
    let id = installations::activate(
        &mut *tx,
        licence.id,
        "q3V2mZ0bTEST0000000001",
        &[2; 32],
        now(),
    )
    .await
    .unwrap();
    run(&mut tx, &["installation", "release", &id.to_string()]).await;
    let row = installations::find(&mut *tx, licence.id, "q3V2mZ0bTEST0000000001")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.status.as_str(), row.released_reason.as_deref()),
        ("released", Some("admin"))
    );
    assert!(
        run_with(&mut tx, &["installation", "release", &id.to_string()], true)
            .await
            .is_err()
    );

    let audit = audit_actions(&mut tx).await;
    let actions: Vec<&str> = audit.iter().map(|a| a.1.as_str()).collect();
    assert_eq!(
        actions,
        [
            "user.block",
            "user.unblock",
            "licence.comp",
            "installation.release"
        ]
    );
    assert_eq!(audit[0].3["user"], user.to_string());
    assert_eq!(audit[3].2, Some(licence.id));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn plans_sync_shows_the_changes_asks_and_records_each_applied_sync() {
    let Some(mut conn) = support::connect().await else {
        return;
    };
    let (mut tx, _) = begin(&mut conn, "plans@example.invalid").await;
    let dir = std::env::temp_dir().join(format!("devtab-plans-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("plans.toml");
    let path = file.to_str().unwrap().to_owned();
    std::fs::write(
        &file,
        "[[plan]]\nid = \"test-sync-free\"\nkind = \"free\"\nsort = 1\n[plan.texts.en]\nname = \"Free\"\n\n\
         [[plan]]\nid = \"test-sync-month\"\nkind = \"subscription\"\ninterval = \"month\"\nseats = 3\nsort = 2\n\
         [plan.texts.en]\nname = \"Monthly\"\n",
    )
    .unwrap();

    let declined = run_with(&mut tx, &["plans", "sync", "--file", &path], false)
        .await
        .unwrap();
    assert!(declined.contains("+ plan test-sync-free"), "{declined}");
    assert!(declined.contains("+ texts test-sync-free/en"), "{declined}");
    assert!(declined.contains("nothing changed"));
    assert!(
        devtab_api::store::plans::find(&mut *tx, "test-sync-free")
            .await
            .unwrap()
            .is_none()
    );

    let applied = run_with(&mut tx, &["plans", "sync", "--file", &path, "--yes"], false)
        .await
        .unwrap();
    assert!(
        applied.contains("written: 2 plan(s), 2 text(s), 0 price(s)"),
        "{applied}"
    );
    let month = devtab_api::store::plans::find(&mut *tx, "test-sync-month")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (month.interval.as_deref(), month.seats, month.available),
        (Some("month"), Some(3), false)
    );

    let again = run(&mut tx, &["plans", "sync", "--file", &path]).await;
    assert!(again.contains("the catalogue is up to date"), "{again}");

    std::fs::write(
        &file,
        "[[plan]]\nid = \"test-sync-month\"\nkind = \"subscription\"\ninterval = \"month\"\nseats = 5\nsort = 2\n\
         [plan.texts.en]\nname = \"Monthly\"\n",
    )
    .unwrap();
    let changed = run(&mut tx, &["plans", "sync", "--file", &path]).await;
    assert!(changed.contains("~ plan test-sync-month"), "{changed}");
    assert!(changed.contains("plan test-sync-free  not in the file: left as it is"));
    let month = devtab_api::store::plans::find(&mut *tx, "test-sync-month")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(month.seats, Some(5));

    std::fs::write(
        &file,
        "[[plan]]\nid = \"x\"\nkind = \"free\"\nsort = 1\nprice = 1\n",
    )
    .unwrap();
    assert!(
        run_with(&mut tx, &["plans", "sync", "--file", &path, "--yes"], true)
            .await
            .is_err()
    );
    std::fs::remove_dir_all(&dir).unwrap();

    let audit = audit_actions(&mut tx).await;
    let synced: Vec<&Value> = audit
        .iter()
        .filter(|a| a.1 == "plans.sync")
        .map(|a| &a.3)
        .collect();
    assert_eq!(synced.len(), 2, "one record per applied sync");
    assert_eq!(synced[1]["plans"], serde_json::json!(["test-sync-month"]));
    tx.rollback().await.unwrap();
}

#[test]
fn the_keys_commands_parse_and_generate_writes_a_new_file_only() {
    assert!(matches!(
        parse(&[
            "keys",
            "generate",
            "--test",
            "--out",
            "x.pem",
            "--list-out",
            "x.json"
        ]),
        AdminCommand::Keys(_)
    ));
    let dir = std::env::temp_dir().join(format!("devtab-keys-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("signing-p9.pem");
    let mut out = Vec::new();
    admin::keys_generate(Some("p9"), false, &path, None, now(), &mut out).unwrap();
    let printed = String::from_utf8(out).unwrap();
    assert!(printed.contains("\"kid\": \"p9\""));
    assert!(
        !printed.contains("PRIVATE KEY"),
        "the private key goes only into the file"
    );
    let pem = std::fs::read_to_string(&path).unwrap();
    assert!(pem.starts_with(concat!("-----BEGIN", " PRIV", "ATE KEY-----")));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(
        admin::keys_generate(Some("p9"), false, &path, None, now(), &mut Vec::new()).is_err(),
        "never overwrites"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// What `script/api-dev` does on a developer's machine: a test key and its
/// key list, accepted by `keys check` in dev only.
#[test]
fn a_local_test_key_is_made_with_its_list_and_trusted_in_dev_only() {
    use devtab_api::config::Env;
    let dir = std::env::temp_dir().join(format!("devtab-keys-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let pem = dir.join("test-key.pem");
    let list = dir.join("test-keys.json");
    let mut out = Vec::new();
    admin::keys_generate(None, true, &pem, Some(&list), now(), &mut out).unwrap();
    let keys: devtab_api::licence::signer::KeyList =
        serde_json::from_str(&std::fs::read_to_string(&list).unwrap()).unwrap();
    let kid = keys.keys[0].kid.clone();
    assert!(kid.starts_with("test-") && keys.keys[0].test);

    let mut out = Vec::new();
    admin::keys_check(
        Env::Dev,
        Some(&pem),
        Some(&kid),
        Some(&list),
        now(),
        &mut out,
    )
    .unwrap();
    assert!(String::from_utf8(out).unwrap().contains("may sign now"));
    let err = admin::keys_check(
        Env::Dev,
        Some(&pem),
        Some(&kid),
        None,
        now(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("no key is published yet"), "{err}");
    for env in [Env::Staging, Env::Prod] {
        let err = admin::keys_check(
            env,
            Some(&pem),
            Some(&kid),
            Some(&list),
            now(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("only in dev"), "{err}");
        let err = admin::keys_check(env, Some(&pem), Some(&kid), None, now(), &mut Vec::new())
            .unwrap_err();
        assert!(err.to_string().contains("test key"), "{err}");
    }
    assert!(
        admin::keys_check(
            Env::Dev,
            None,
            Some(&kid),
            Some(&list),
            now(),
            &mut Vec::new()
        )
        .is_err()
    );
    assert!(
        admin::keys_generate(
            None,
            true,
            &dir.join("other.pem"),
            Some(&list),
            now(),
            &mut Vec::new()
        )
        .is_err(),
        "never overwrites the list"
    );
    assert!(
        !dir.join("other.pem").exists(),
        "nothing written when the list exists"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
