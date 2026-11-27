//! `app.key_reveals`: a new licence's key, kept in plain text only until it
//! is shown once to the buyer (and mailed), or a day has passed.

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

pub async fn insert(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
    key_plain: &str,
    now: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "insert into app.key_reveals (licence_id, key_plain, created_at, expires_at)
         values ($1, $2, $3, $4)",
        licence_id,
        key_plain,
        now,
        expires_at,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// The key, the first time it is asked for before it expires; `None` ever
/// after.
pub async fn reveal_once(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
    now: OffsetDateTime,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!(
        "update app.key_reveals set revealed_at = $2
         where licence_id = $1 and revealed_at is null and expires_at > $2
         returning key_plain",
        licence_id,
        now,
    )
    .fetch_optional(db)
    .await
}
