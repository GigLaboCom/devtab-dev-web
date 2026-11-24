//! `app.jobs`: side effects to run later (an outbox). A job is claimed with
//! `for update skip locked`, so that two workers never run the same one.
//! The payload holds record ids only.

use serde_json::Value;
use sqlx::PgExecutor;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct JobRow {
    pub id: i64,
    pub kind: String,
    pub payload: Value,
    pub run_at: OffsetDateTime,
    pub attempts: i32,
}

pub async fn enqueue(
    db: impl PgExecutor<'_>,
    kind: &str,
    payload: &Value,
    run_at: OffsetDateTime,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        "insert into app.jobs (kind, payload, run_at) values ($1, $2, $3) returning id",
        kind,
        payload,
        run_at,
    )
    .fetch_one(db)
    .await
}

/// Up to `limit` jobs due at `now`, locked for the caller's transaction and
/// skipped by every other one.
pub async fn claim_due(
    db: impl PgExecutor<'_>,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<JobRow>, sqlx::Error> {
    sqlx::query_as!(
        JobRow,
        "select id, kind, payload, run_at, attempts from app.jobs
         where done_at is null and run_at <= $1
         order by run_at, id
         limit $2
         for update skip locked",
        now,
        limit,
    )
    .fetch_all(db)
    .await
}

pub async fn complete(
    db: impl PgExecutor<'_>,
    id: i64,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.jobs set done_at = $2, attempts = attempts + 1, last_error = null where id = $1",
        id,
        now,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// A failed attempt; the job runs again at `retry_at`. `error` carries no
/// personal data.
pub async fn fail(
    db: impl PgExecutor<'_>,
    id: i64,
    error: &str,
    retry_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.jobs set attempts = attempts + 1, last_error = $2, run_at = $3 where id = $1",
        id,
        error,
        retry_at,
    )
    .execute(db)
    .await
    .map(|_| ())
}
