//! `app.licence_members`: the people a team's owner invited by e-mail. The
//! owner is the licence's user and the first member; the others become
//! themselves once they sign in with the address and connect.

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRow {
    pub email: String,
    pub user_id: Option<Uuid>,
    pub invited_at: OffsetDateTime,
}

pub async fn of_licence(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
) -> Result<Vec<MemberRow>, sqlx::Error> {
    sqlx::query_as!(
        MemberRow,
        "select email, user_id, invited_at from app.licence_members
         where licence_id = $1 order by invited_at, email",
        licence_id,
    )
    .fetch_all(db)
    .await
}

/// Invites `email` (lower case): the member's id; none when already invited.
pub async fn add(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
    email: &str,
    now: OffsetDateTime,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "insert into app.licence_members (licence_id, email, invited_at) values ($1, $2, $3)
         on conflict (licence_id, email) do nothing
         returning id",
        licence_id,
        email,
        now,
    )
    .fetch_optional(db)
    .await
}

/// A member by id: their licence and address.
pub async fn find_by_id(
    db: impl PgExecutor<'_>,
    id: Uuid,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    let row = sqlx::query!(
        "select licence_id, email from app.licence_members where id = $1",
        id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| (r.licence_id, r.email)))
}

/// Removes the member; the person they became, if any; `None` when there
/// was no such member.
pub async fn remove(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
    email: &str,
) -> Result<Option<Option<Uuid>>, sqlx::Error> {
    sqlx::query_scalar!(
        "delete from app.licence_members where licence_id = $1 and email = $2 returning user_id",
        licence_id,
        email,
    )
    .fetch_optional(db)
    .await
}

/// Records which person the invited address became.
pub async fn join(
    db: impl PgExecutor<'_>,
    licence_id: Uuid,
    email: &str,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.licence_members set user_id = $3
         where licence_id = $1 and email = $2 and user_id is distinct from $3",
        licence_id,
        email,
        user_id,
    )
    .execute(db)
    .await
    .map(|_| ())
}
