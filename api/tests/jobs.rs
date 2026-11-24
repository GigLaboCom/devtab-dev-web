//! The scheduler's task that frees the seats of installations not seen for
//! 60 days, with time moved by a clock instead of waited for, against the
//! local Supabase database (`DEVTAB_TEST_DATABASE_URL`).
#![allow(clippy::unwrap_used)]

mod support;

use std::sync::Arc;

use devtab_api::clock::ManualClock;
use devtab_api::jobs::RELEASE_INACTIVE_LOCK;
use devtab_api::jobs::scheduler::{self, Run};
use devtab_api::licence::{LicenceSettings, key, seats};
use devtab_api::store::installations;
use devtab_api::store::licences::{self, NewLicence};
use devtab_api::{AppState, Config};
use sqlx::{Connection as _, PgConnection};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp()).unwrap()
}

async fn complimentary(conn: &mut PgConnection) -> Uuid {
    let new_key = key::generate().unwrap();
    let lid = key::new_lid().unwrap();
    licences::insert(
        conn,
        &NewLicence {
            lid: &lid,
            user_id: None,
            customer_id: None,
            plan_id: "test-monthly",
            kind: "complimentary",
            seats: 2,
            paid_until: None,
            cancel_at_period_end: false,
            provider: None,
            provider_subscription_id: None,
            key_hash: &new_key.hash,
            key_hint: &new_key.hint,
            note: None,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn an_installation_loses_its_seat_after_60_days_without_contact() {
    let Some(mut conn) = support::connect().await else {
        return;
    };
    let mut tx = conn.begin().await.unwrap();
    support::ensure_test_plans(&mut tx).await;
    sqlx::query("set local role devtab_api")
        .execute(&mut *tx)
        .await
        .unwrap();
    // Far enough in the past that no cutoff below reaches rows of tests
    // running at the same time.
    let start = now() - Duration::days(200);
    let licence = complimentary(&mut tx).await;
    let quiet =
        installations::activate(&mut *tx, licence, "q3V2mZ0bTEST000000000a", &[1; 32], start)
            .await
            .unwrap();
    let busy =
        installations::activate(&mut *tx, licence, "q3V2mZ0bTEST000000000b", &[2; 32], start)
            .await
            .unwrap();
    let settings = LicenceSettings::default();
    let clock = ManualClock::new(start);

    clock.advance(Duration::days(30));
    installations::touch(&mut *tx, busy, devtab_api::clock::Clock::now(&clock))
        .await
        .unwrap();

    clock.advance(Duration::days(29) + Duration::hours(23));
    let released =
        seats::release_inactive(&mut tx, devtab_api::clock::Clock::now(&clock), &settings)
            .await
            .unwrap();
    assert!(
        !released.iter().any(|r| r.licence_id == licence),
        "59 days and 23 hours: kept"
    );

    clock.advance(Duration::hours(2));
    let at = devtab_api::clock::Clock::now(&clock);
    let released = seats::release_inactive(&mut tx, at, &settings)
        .await
        .unwrap();
    let ours: Vec<Uuid> = released
        .iter()
        .filter(|r| r.licence_id == licence)
        .map(|r| r.id)
        .collect();
    assert_eq!(
        ours,
        [quiet],
        "60 days without contact: released; the busy one stays"
    );
    let row = installations::find(&mut *tx, licence, "q3V2mZ0bTEST000000000a")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            row.status.as_str(),
            row.released_reason.as_deref(),
            row.released_at
        ),
        ("released", Some("inactive"), Some(at))
    );
    assert_eq!(
        installations::count_active(&mut *tx, licence)
            .await
            .unwrap(),
        1
    );

    // DEVTAB_RELEASE_AFTER_DAYS moves the threshold.
    let longer = LicenceSettings {
        release_after_days: 90,
        ..settings
    };
    clock.advance(Duration::days(31));
    let released = seats::release_inactive(&mut tx, devtab_api::clock::Clock::now(&clock), &longer)
        .await
        .unwrap();
    assert!(
        !released.iter().any(|r| r.licence_id == licence),
        "busy: 61 days, under 90"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_task_runs_under_its_advisory_lock_and_skips_while_another_holds_it() {
    let Some(url) = support::database_url() else {
        return;
    };
    let mut owner = support::connect().await.unwrap();
    support::ensure_test_plans(&mut owner).await;
    let licence = complimentary(&mut owner).await;
    let long_ago = now() - Duration::days(61);
    installations::activate(
        &mut owner,
        licence,
        "q3V2mZ0bTEST000000000c",
        &[3; 32],
        long_ago,
    )
    .await
    .unwrap();

    let clock = ManualClock::new(now());
    let state: AppState = support::api_state(&url, Arc::new(clock), &Config::default()).await;

    // Another process holds the lock: this one skips.
    let mut other = PgConnection::connect(&url).await.unwrap();
    let mut held = other.begin().await.unwrap();
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(RELEASE_INACTIVE_LOCK)
        .execute(&mut *held)
        .await
        .unwrap();
    assert_eq!(
        scheduler::release_inactive(&state).await.unwrap(),
        Run::Skipped
    );
    let row = installations::find(&mut owner, licence, "q3V2mZ0bTEST000000000c")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "active");
    held.rollback().await.unwrap();

    // The lock is free: the run releases the installation.
    let Run::Done(released) = scheduler::release_inactive(&state).await.unwrap() else {
        panic!("the lock was free");
    };
    assert!(released.iter().any(|r| r.licence_id == licence));
    let row = installations::find(&mut owner, licence, "q3V2mZ0bTEST000000000c")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.status.as_str(), row.released_reason.as_deref()),
        ("released", Some("inactive"))
    );

    scheduler::tick(&state).await;
    sqlx::query("delete from app.licences where id = $1")
        .bind(licence)
        .execute(&mut owner)
        .await
        .unwrap();
}
