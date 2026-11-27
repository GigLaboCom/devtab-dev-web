//! `app.provider_events`: every event a payment provider delivered with a
//! valid signature, stored before it is answered and processed after.

use serde_json::Value;
use sqlx::PgExecutor;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub id: i64,
    pub provider: String,
    pub event_id: String,
    pub event_type: String,
    pub status: String,
    pub attempts: i32,
    pub payload: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct NewEvent<'a> {
    pub provider: &'a str,
    pub event_id: &'a str,
    pub event_type: &'a str,
    pub occurred_at: OffsetDateTime,
    pub received_at: OffsetDateTime,
    pub payload: &'a Value,
}

/// Stores the event; `None` when the provider delivered it before.
pub async fn insert(
    db: impl PgExecutor<'_>,
    new: &NewEvent<'_>,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!(
        "insert into app.provider_events
           (provider, event_id, event_type, occurred_at, received_at, payload)
         values ($1, $2, $3, $4, $5, $6)
         on conflict (provider, event_id) do nothing
         returning id",
        new.provider,
        new.event_id,
        new.event_type,
        new.occurred_at,
        new.received_at,
        new.payload,
    )
    .fetch_optional(db)
    .await
}

/// The event, locked for the rest of the transaction.
pub async fn find_for_update(
    db: impl PgExecutor<'_>,
    id: i64,
) -> Result<Option<EventRow>, sqlx::Error> {
    sqlx::query_as!(
        EventRow,
        "select id, provider, event_id, event_type, status, attempts, payload
         from app.provider_events where id = $1
         for update",
        id,
    )
    .fetch_optional(db)
    .await
}

/// The outcome of processing: `processed` or `ignored`.
pub async fn finish(
    db: impl PgExecutor<'_>,
    id: i64,
    status: &str,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.provider_events
         set status = $2, processed_at = $3, attempts = attempts + 1, last_error = null
         where id = $1",
        id,
        status,
        now,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// A failed attempt; `failed` once it will not be tried again. `error`
/// carries no personal data.
pub async fn fail(
    db: impl PgExecutor<'_>,
    id: i64,
    error: &str,
    give_up: bool,
) -> Result<(), sqlx::Error> {
    let status = if give_up { "failed" } else { "received" };
    sqlx::query!(
        "update app.provider_events
         set attempts = attempts + 1, last_error = left($2, 1000), status = $3
         where id = $1",
        id,
        error,
        status,
    )
    .execute(db)
    .await
    .map(|_| ())
}
