//! `app.admin_audit`: every change an administrator makes. Append-only: the
//! API role may insert and read, not update or delete. Details never carry
//! personal data.

use serde_json::Value;
use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub struct AuditRow {
    pub id: i64,
    pub at: OffsetDateTime,
    pub actor: String,
    pub action: String,
    pub licence_id: Option<Uuid>,
    pub details: Value,
}

pub async fn record(
    db: impl PgExecutor<'_>,
    actor: &str,
    action: &str,
    licence_id: Option<Uuid>,
    details: &Value,
    at: OffsetDateTime,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        "insert into app.admin_audit (at, actor, action, licence_id, details)
         values ($1, $2, $3, $4, $5)
         returning id",
        at,
        actor,
        action,
        licence_id,
        details,
    )
    .fetch_one(db)
    .await
}

/// The latest entries of a licence, newest first.
pub async fn of_licence(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
    limit: i64,
) -> Result<Vec<AuditRow>, sqlx::Error> {
    sqlx::query_as!(
        AuditRow,
        "select id, at, actor, action, licence_id, details from app.admin_audit
         where licence_id = $1
         order by at desc, id desc
         limit $2",
        licence_id,
        limit,
    )
    .fetch_all(db)
    .await
}
