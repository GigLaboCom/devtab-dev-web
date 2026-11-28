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

/// Queues a job once: none is added when a job with `dedupe_key` was ever
/// queued (done or not). True when this one was added.
pub async fn enqueue_once(
    db: impl PgExecutor<'_>,
    kind: &str,
    payload: &Value,
    run_at: OffsetDateTime,
    dedupe_key: &str,
) -> Result<bool, sqlx::Error> {
    let added = sqlx::query!(
        "insert into app.jobs (kind, payload, run_at, dedupe_key) values ($1, $2, $3, $4)
         on conflict (dedupe_key) where dedupe_key is not null do nothing",
        kind,
        payload,
        run_at,
        dedupe_key,
    )
    .execute(db)
    .await?;
    Ok(added.rows_affected() == 1)
}

/// Up to `limit` jobs of the `kinds` the caller runs, due at `now`, locked
/// for the caller's transaction and skipped by every other one. A job of
/// another kind waits in the queue for a worker that runs it.
pub async fn claim_due(
    db: impl PgExecutor<'_>,
    now: OffsetDateTime,
    limit: i64,
    kinds: &[&str],
) -> Result<Vec<JobRow>, sqlx::Error> {
    let kinds: Vec<String> = kinds.iter().map(|&kind| kind.to_owned()).collect();
    sqlx::query_as!(
        JobRow,
        "select id, kind, payload, run_at, attempts from app.jobs
         where done_at is null and run_at <= $1 and kind = any($3)
         order by run_at, id
         limit $2
         for update skip locked",
        now,
        limit,
        &kinds,
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

/// A job given up on: done, with the error that stopped it.
pub async fn give_up(
    db: impl PgExecutor<'_>,
    id: i64,
    error: &str,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.jobs set done_at = $3, attempts = attempts + 1, last_error = $2 where id = $1",
        id,
        error,
        now,
    )
    .execute(db)
    .await
    .map(|_| ())
}
