//! `app.checkouts`: a purchase a signed-in person started. Only that person
//! may read it; the key it led to is shown to them once.

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub plan_id: String,
    pub price_id: Uuid,
    pub provider: String,
    pub provider_checkout_id: Option<String>,
    pub licence_id: Option<Uuid>,
    pub status: String,
    pub locale: String,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct NewCheckout<'a> {
    pub user_id: Uuid,
    pub plan_id: &'a str,
    pub price_id: Uuid,
    pub provider: &'a str,
    pub locale: &'a str,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

pub async fn insert(db: impl PgExecutor<'_>, new: &NewCheckout<'_>) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar!(
        "insert into app.checkouts (user_id, plan_id, price_id, provider, locale, created_at, expires_at)
         values ($1, $2, $3, $4, $5, $6, $7)
         returning id",
        new.user_id,
        new.plan_id,
        new.price_id,
        new.provider,
        new.locale,
        new.created_at,
        new.expires_at,
    )
    .fetch_one(db)
    .await
}

/// Records the provider's id of the checkout it started.
pub async fn set_provider_checkout(
    db: impl PgExecutor<'_>,
    id: Uuid,
    provider_checkout_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.checkouts set provider_checkout_id = $2 where id = $1",
        id,
        provider_checkout_id,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// The checkout `id` if `user_id` started it.
pub async fn find_of_user(
    db: impl PgExecutor<'_>,
    id: Uuid,
    user_id: Uuid,
) -> Result<Option<CheckoutRow>, sqlx::Error> {
    sqlx::query_as!(
        CheckoutRow,
        "select id, user_id, plan_id, price_id, provider, provider_checkout_id, licence_id, status,
                locale, created_at, expires_at
         from app.checkouts where id = $1 and user_id = $2",
        id,
        user_id,
    )
    .fetch_optional(db)
    .await
}

/// The checkout, locked for the rest of the transaction.
pub async fn find_for_update(
    db: impl PgExecutor<'_>,
    id: Uuid,
) -> Result<Option<CheckoutRow>, sqlx::Error> {
    sqlx::query_as!(
        CheckoutRow,
        "select id, user_id, plan_id, price_id, provider, provider_checkout_id, licence_id, status,
                locale, created_at, expires_at
         from app.checkouts where id = $1
         for update",
        id,
    )
    .fetch_optional(db)
    .await
}

/// Marks the checkout completed by `licence_id`.
pub async fn complete(
    db: impl PgExecutor<'_>,
    id: Uuid,
    licence_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.checkouts set status = 'completed', licence_id = $2, completed_at = $3
         where id = $1",
        id,
        licence_id,
        now,
    )
    .execute(db)
    .await
    .map(|_| ())
}
