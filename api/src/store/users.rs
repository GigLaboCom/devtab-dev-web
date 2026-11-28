//! `app.users`: one row per Supabase Auth user. The e-mail address lives in
//! `auth.users`, which the API role reads only through `app.user_email` and
//! `app.user_id_by_email`; `app.users` keeps only its canonical inbox
//! (`email_canonical`, set by a trigger on `auth.users`).

use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    pub id: Uuid,
    pub status: String,
}

pub async fn find(db: impl PgExecutor<'_>, id: Uuid) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as!(
        UserRow,
        "select id, status from app.users where id = $1",
        id,
    )
    .fetch_optional(db)
    .await
}

/// The user with this e-mail address, through `app.user_id_by_email`.
pub async fn id_by_email(
    db: impl PgExecutor<'_>,
    email: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!("select app.user_id_by_email($1) as id", email)
        .fetch_one(db)
        .await
}

/// The first user whose address reaches the inbox `canonical`
/// (`accounts::canonical_email`), by `app.users.email_canonical`.
pub async fn id_by_canonical_email(
    db: impl PgExecutor<'_>,
    canonical: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "select id from app.users where email_canonical = $1 order by created_at, id limit 1",
        canonical,
    )
    .fetch_optional(db)
    .await
}

/// The user's e-mail address, through `app.user_email`.
pub async fn email_of(db: impl PgExecutor<'_>, id: Uuid) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!("select app.user_email($1) as email", id)
        .fetch_one(db)
        .await
}

/// Sets `active` or `blocked`; false when there is no such user.
pub async fn set_status(
    db: impl PgExecutor<'_>,
    id: Uuid,
    status: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("update app.users set status = $2 where id = $1", id, status,)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}
