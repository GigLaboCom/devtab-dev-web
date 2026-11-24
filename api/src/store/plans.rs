//! `app.plans`: what `devtab-api admin plans sync` writes and a licence is
//! issued on.

use sqlx::PgExecutor;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRow {
    pub id: String,
    pub kind: String,
    pub interval: Option<String>,
    pub seats: Option<i32>,
    pub sort: i16,
    pub available: bool,
    pub until: Option<OffsetDateTime>,
}

pub async fn all(db: impl PgExecutor<'_>) -> Result<Vec<PlanRow>, sqlx::Error> {
    sqlx::query_as!(
        PlanRow,
        r#"select id, kind, "interval", seats, sort, available, until
           from app.plans order by sort, id"#,
    )
    .fetch_all(db)
    .await
}

pub async fn find(db: impl PgExecutor<'_>, id: &str) -> Result<Option<PlanRow>, sqlx::Error> {
    sqlx::query_as!(
        PlanRow,
        r#"select id, kind, "interval", seats, sort, available, until
           from app.plans where id = $1"#,
        id,
    )
    .fetch_optional(db)
    .await
}

/// Creates the plan or updates every column of it.
pub async fn upsert(db: impl PgExecutor<'_>, plan: &PlanRow) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"insert into app.plans (id, kind, "interval", seats, sort, available, until)
           values ($1, $2, $3, $4, $5, $6, $7)
           on conflict (id) do update set
             kind = excluded.kind, "interval" = excluded."interval", seats = excluded.seats,
             sort = excluded.sort, available = excluded.available, until = excluded.until"#,
        plan.id,
        plan.kind,
        plan.interval,
        plan.seats,
        plan.sort,
        plan.available,
        plan.until,
    )
    .execute(db)
    .await
    .map(|_| ())
}
