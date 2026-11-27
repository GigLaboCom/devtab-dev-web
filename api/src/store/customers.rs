//! `app.customers`: a user as the customer of a payment provider. Rows come
//! from verified provider events; the provider's customer id is a pseudonym.

use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerRow {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub provider: String,
    pub provider_customer_id: String,
}

pub async fn insert(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
    provider: &str,
    provider_customer_id: &str,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar!(
        "insert into app.customers (user_id, provider, provider_customer_id)
         values ($1, $2, $3)
         returning id",
        user_id,
        provider,
        provider_customer_id,
    )
    .fetch_one(db)
    .await
}

pub async fn find(db: impl PgExecutor<'_>, id: Uuid) -> Result<Option<CustomerRow>, sqlx::Error> {
    sqlx::query_as!(
        CustomerRow,
        "select id, user_id, provider, provider_customer_id from app.customers where id = $1",
        id,
    )
    .fetch_optional(db)
    .await
}

/// The customer the provider knows by `provider_customer_id`.
pub async fn find_by_provider_id(
    db: impl PgExecutor<'_>,
    provider: &str,
    provider_customer_id: &str,
) -> Result<Option<CustomerRow>, sqlx::Error> {
    sqlx::query_as!(
        CustomerRow,
        "select id, user_id, provider, provider_customer_id from app.customers
         where provider = $1 and provider_customer_id = $2",
        provider,
        provider_customer_id,
    )
    .fetch_optional(db)
    .await
}

/// The user's customer at `provider`.
pub async fn find_of_user(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
    provider: &str,
) -> Result<Option<CustomerRow>, sqlx::Error> {
    sqlx::query_as!(
        CustomerRow,
        "select id, user_id, provider, provider_customer_id from app.customers
         where user_id = $1 and provider = $2",
        user_id,
        provider,
    )
    .fetch_optional(db)
    .await
}
