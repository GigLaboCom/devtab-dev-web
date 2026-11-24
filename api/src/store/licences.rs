//! `app.licences`. A licence is found by the hash of its key (activation)
//! or by its `lid` (refresh, deactivate, admin commands); the key itself is
//! never stored.

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

/// A licence with its plan's billing period (`term`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenceRow {
    pub id: Uuid,
    pub lid: String,
    pub user_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub plan_id: String,
    pub kind: String,
    pub status: String,
    pub revoke_reason: Option<String>,
    pub seats: i32,
    pub paid_until: Option<OffsetDateTime>,
    pub cancel_at_period_end: bool,
    pub key_hint: String,
    pub key_version: i32,
    pub key_blocked_at: Option<OffsetDateTime>,
    pub note: Option<String>,
    pub created_at: OffsetDateTime,
    pub term: Option<String>,
}

/// A licence to insert. Complimentary licences have no customer and no
/// provider; the others have both.
#[derive(Debug, Clone)]
pub struct NewLicence<'a> {
    pub lid: &'a str,
    pub user_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub plan_id: &'a str,
    pub kind: &'a str,
    pub seats: i32,
    pub paid_until: Option<OffsetDateTime>,
    pub cancel_at_period_end: bool,
    pub provider: Option<&'a str>,
    pub provider_subscription_id: Option<&'a str>,
    pub key_hash: &'a [u8],
    pub key_hint: &'a str,
    pub note: Option<&'a str>,
}

pub async fn insert(db: impl PgExecutor<'_>, new: &NewLicence<'_>) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar!(
        "insert into app.licences
           (lid, user_id, customer_id, plan_id, kind, seats, paid_until, cancel_at_period_end,
            provider, provider_subscription_id, key_hash, key_hint, note)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         returning id",
        new.lid,
        new.user_id,
        new.customer_id,
        new.plan_id,
        new.kind,
        new.seats,
        new.paid_until,
        new.cancel_at_period_end,
        new.provider,
        new.provider_subscription_id,
        new.key_hash,
        new.key_hint,
        new.note,
    )
    .fetch_one(db)
    .await
}

/// The licence of a key hash, locked for the rest of the transaction.
pub async fn find_by_key_hash_for_update(
    db: impl PgExecutor<'_>,
    key_hash: &[u8],
) -> Result<Option<LicenceRow>, sqlx::Error> {
    sqlx::query_as!(
        LicenceRow,
        r#"select l.id, l.lid, l.user_id, l.customer_id, l.plan_id, l.kind, l.status,
                  l.revoke_reason, l.seats, l.paid_until, l.cancel_at_period_end, l.key_hint,
                  l.key_version, l.key_blocked_at, l.note, l.created_at, p."interval" as "term?"
           from app.licences l join app.plans p on p.id = l.plan_id
           where l.key_hash = $1
           for update of l"#,
        key_hash,
    )
    .fetch_optional(db)
    .await
}

pub async fn find_by_lid(
    db: impl PgExecutor<'_>,
    lid: &str,
) -> Result<Option<LicenceRow>, sqlx::Error> {
    sqlx::query_as!(
        LicenceRow,
        r#"select l.id, l.lid, l.user_id, l.customer_id, l.plan_id, l.kind, l.status,
                  l.revoke_reason, l.seats, l.paid_until, l.cancel_at_period_end, l.key_hint,
                  l.key_version, l.key_blocked_at, l.note, l.created_at, p."interval" as "term?"
           from app.licences l join app.plans p on p.id = l.plan_id
           where l.lid = $1"#,
        lid,
    )
    .fetch_optional(db)
    .await
}

/// The licence with this `lid`, locked for the rest of the transaction.
pub async fn find_by_lid_for_update(
    db: impl PgExecutor<'_>,
    lid: &str,
) -> Result<Option<LicenceRow>, sqlx::Error> {
    sqlx::query_as!(
        LicenceRow,
        r#"select l.id, l.lid, l.user_id, l.customer_id, l.plan_id, l.kind, l.status,
                  l.revoke_reason, l.seats, l.paid_until, l.cancel_at_period_end, l.key_hint,
                  l.key_version, l.key_blocked_at, l.note, l.created_at, p."interval" as "term?"
           from app.licences l join app.plans p on p.id = l.plan_id
           where l.lid = $1
           for update of l"#,
        lid,
    )
    .fetch_optional(db)
    .await
}

/// The user's licences, newest first.
pub async fn of_user(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
) -> Result<Vec<LicenceRow>, sqlx::Error> {
    sqlx::query_as!(
        LicenceRow,
        r#"select l.id, l.lid, l.user_id, l.customer_id, l.plan_id, l.kind, l.status,
                  l.revoke_reason, l.seats, l.paid_until, l.cancel_at_period_end, l.key_hint,
                  l.key_version, l.key_blocked_at, l.note, l.created_at, p."interval" as "term?"
           from app.licences l join app.plans p on p.id = l.plan_id
           where l.user_id = $1
           order by l.created_at desc, l.lid"#,
        user_id,
    )
    .fetch_all(db)
    .await
}

/// Blocks the key: the licence is revoked (`key-blocked`, unless it was
/// already revoked for a refund) and stays so.
pub async fn block_key(
    db: impl PgExecutor<'_>,
    id: Uuid,
    at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.licences set
           key_blocked_at = coalesce(key_blocked_at, $2),
           revoke_reason = case when status = 'revoked' then revoke_reason else 'key-blocked' end,
           status = 'revoked',
           revoked_at = coalesce(revoked_at, $2)
         where id = $1",
        id,
        at,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// Records a refund: the licence is revoked (`refund`).
pub async fn revoke_for_refund(
    db: impl PgExecutor<'_>,
    id: Uuid,
    at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.licences set
           status = 'revoked', revoke_reason = 'refund', revoked_at = coalesce(revoked_at, $2)
         where id = $1",
        id,
        at,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// Replaces the key; returns the new key version. Installations activated
/// with the old key keep working: they have their own secrets.
pub async fn replace_key(
    db: impl PgExecutor<'_>,
    id: Uuid,
    key_hash: &[u8],
    key_hint: &str,
) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        "update app.licences set key_hash = $2, key_hint = $3, key_version = key_version + 1
         where id = $1
         returning key_version",
        id,
        key_hash,
        key_hint,
    )
    .fetch_one(db)
    .await
}
