//! `app.installations`: the seats of a licence. Times come from the caller's
//! clock, not from the database's `now()`, so that tests can move time.

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationRow {
    pub id: Uuid,
    pub licence_id: Uuid,
    pub inst: String,
    pub secret_hash: Vec<u8>,
    pub status: String,
    pub released_reason: Option<String>,
    pub activated_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub released_at: Option<OffsetDateTime>,
}

/// An installation that lost its seat for want of contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Released {
    pub id: Uuid,
    pub licence_id: Uuid,
}

pub async fn find(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
    inst: &str,
) -> Result<Option<InstallationRow>, sqlx::Error> {
    sqlx::query_as!(
        InstallationRow,
        "select id, licence_id, inst, secret_hash, status, released_reason, activated_at,
                last_seen_at, released_at
         from app.installations where licence_id = $1 and inst = $2",
        licence_id,
        inst,
    )
    .fetch_optional(db)
    .await
}

pub async fn of_licence(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
) -> Result<Vec<InstallationRow>, sqlx::Error> {
    sqlx::query_as!(
        InstallationRow,
        "select id, licence_id, inst, secret_hash, status, released_reason, activated_at,
                last_seen_at, released_at
         from app.installations where licence_id = $1
         order by activated_at, id",
        licence_id,
    )
    .fetch_all(db)
    .await
}

pub async fn count_active(db: impl PgExecutor<'_>, licence_id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        r#"select count(*) as "n!" from app.installations
           where licence_id = $1 and status = 'active'"#,
        licence_id,
    )
    .fetch_one(db)
    .await
}

/// Installations of the licence that took a seat after `since`.
pub async fn count_activated_since(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
    since: OffsetDateTime,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        r#"select count(*) as "n!" from app.installations
           where licence_id = $1 and activated_at > $2"#,
        licence_id,
        since,
    )
    .fetch_one(db)
    .await
}

/// Takes a seat: a new installation, or a released or deactivated one back
/// in `active` with a new secret.
pub async fn activate(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
    inst: &str,
    secret_hash: &[u8],
    now: OffsetDateTime,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar!(
        "insert into app.installations
           (licence_id, inst, secret_hash, status, activated_at, last_seen_at)
         values ($1, $2, $3, 'active', $4, $4)
         on conflict (licence_id, inst) do update set
           secret_hash = excluded.secret_hash, status = 'active', released_reason = null,
           released_at = null, activated_at = excluded.activated_at,
           last_seen_at = excluded.last_seen_at
         returning id",
        licence_id,
        inst,
        secret_hash,
        now,
    )
    .fetch_one(db)
    .await
}

/// The same active installation activated again: a new secret, no new seat.
pub async fn replace_secret(
    db: impl PgExecutor<'_>,
    id: Uuid,
    secret_hash: &[u8],
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.installations set secret_hash = $2, last_seen_at = $3 where id = $1",
        id,
        secret_hash,
        now,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// Records contact; an installation released for want of contact comes back.
pub async fn touch(
    db: impl PgExecutor<'_>,
    id: Uuid,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.installations set
           last_seen_at = $2, status = 'active', released_reason = null, released_at = null
         where id = $1",
        id,
        now,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// Deactivated by the extension: the seat is free.
pub async fn deactivate(
    db: impl PgExecutor<'_>,
    id: Uuid,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.installations set
           status = 'deactivated', released_reason = null, released_at = $2
         where id = $1 and status <> 'deactivated'",
        id,
        now,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// Frees the seat of an active installation (`manual` from the site, `admin`
/// from the command line). Returns the licence, or `None` when there is no
/// such active installation.
pub async fn release(
    db: impl PgExecutor<'_>,
    id: Uuid,
    reason: &str,
    now: OffsetDateTime,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "update app.installations set
           status = 'released', released_reason = $2, released_at = $3
         where id = $1 and status = 'active'
         returning licence_id",
        id,
        reason,
        now,
    )
    .fetch_optional(db)
    .await
}

/// Frees the seats of active installations not seen since `cutoff`.
pub async fn release_inactive(
    db: impl PgExecutor<'_>,
    cutoff: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<Vec<Released>, sqlx::Error> {
    sqlx::query_as!(
        Released,
        "update app.installations set
           status = 'released', released_reason = 'inactive', released_at = $2
         where status = 'active' and last_seen_at < $1
         returning id, licence_id",
        cutoff,
        now,
    )
    .fetch_all(db)
    .await
}

/// Records the person who connected the installation from their account.
pub async fn set_user(db: impl PgExecutor<'_>, id: Uuid, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.installations set user_id = $2 where id = $1",
        id,
        user_id
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// Frees the seats of the installations `user_id` connected to the licence
/// from their account; how many.
pub async fn release_of_user(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
    user_id: Uuid,
    now: OffsetDateTime,
) -> Result<u64, sqlx::Error> {
    let released = sqlx::query!(
        "update app.installations set
           status = 'released', released_reason = 'manual', released_at = $3
         where licence_id = $1 and user_id = $2 and status = 'active'",
        licence_id,
        user_id,
        now,
    )
    .execute(db)
    .await?;
    Ok(released.rows_affected())
}
