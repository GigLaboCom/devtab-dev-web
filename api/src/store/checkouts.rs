//! `app.checkouts`: a purchase a signed-in person started, or anyone without
//! an account (a guest checkout, whose buyer becomes a user when it is
//! paid). Only the person who started it, or the browser that holds a guest
//! checkout's secret, may read it; the key it led to is shown once.

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRow {
    pub id: Uuid,
    /// None for a guest checkout until its buyer has an account.
    pub user_id: Option<Uuid>,
    pub plan_id: String,
    pub price_id: Uuid,
    pub provider: String,
    pub provider_checkout_id: Option<String>,
    pub licence_id: Option<Uuid>,
    pub status: String,
    pub locale: String,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    /// A guest checkout: SHA-256 of the starting browser's secret.
    pub claim_hash: Option<Vec<u8>>,
    /// A guest checkout's buyer: `invited` or `existing`, once paid.
    pub buyer: Option<String>,
}

impl CheckoutRow {
    /// Started without a signed-in person.
    pub fn is_guest(&self) -> bool {
        self.claim_hash.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct NewCheckout<'a> {
    /// The signed-in person; none for a guest checkout, which has a
    /// `claim_hash` instead.
    pub user_id: Option<Uuid>,
    pub plan_id: &'a str,
    pub price_id: Uuid,
    pub provider: &'a str,
    pub locale: &'a str,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub claim_hash: Option<&'a [u8]>,
}

pub async fn insert(db: impl PgExecutor<'_>, new: &NewCheckout<'_>) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar!(
        "insert into app.checkouts
           (user_id, plan_id, price_id, provider, locale, created_at, expires_at, claim_hash)
         values ($1, $2, $3, $4, $5, $6, $7, $8)
         returning id",
        new.user_id,
        new.plan_id,
        new.price_id,
        new.provider,
        new.locale,
        new.created_at,
        new.expires_at,
        new.claim_hash,
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
                locale, created_at, expires_at, claim_hash, buyer
         from app.checkouts where id = $1 and user_id = $2",
        id,
        user_id,
    )
    .fetch_optional(db)
    .await
}

/// The guest checkout `id` if `claim_hash` is its starting browser's.
pub async fn find_by_claim(
    db: impl PgExecutor<'_>,
    id: Uuid,
    claim_hash: &[u8],
) -> Result<Option<CheckoutRow>, sqlx::Error> {
    sqlx::query_as!(
        CheckoutRow,
        "select id, user_id, plan_id, price_id, provider, provider_checkout_id, licence_id, status,
                locale, created_at, expires_at, claim_hash, buyer
         from app.checkouts where id = $1 and claim_hash = $2",
        id,
        claim_hash,
    )
    .fetch_optional(db)
    .await
}

/// Gives a guest checkout its buyer: the user it now belongs to, `invited`
/// or `existing`.
pub async fn set_buyer(
    db: impl PgExecutor<'_>,
    id: Uuid,
    user_id: Uuid,
    buyer: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.checkouts set user_id = $2, buyer = $3 where id = $1",
        id,
        user_id,
        buyer,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// The checkout, locked for the rest of the transaction.
pub async fn find_for_update(
    db: impl PgExecutor<'_>,
    id: Uuid,
) -> Result<Option<CheckoutRow>, sqlx::Error> {
    sqlx::query_as!(
        CheckoutRow,
        "select id, user_id, plan_id, price_id, provider, provider_checkout_id, licence_id, status,
                locale, created_at, expires_at, claim_hash, buyer
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
