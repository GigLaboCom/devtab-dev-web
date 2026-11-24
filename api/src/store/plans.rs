//! `app.plans`, `app.plan_texts`, `app.plan_prices` and the catalogue's
//! version in `app.settings`: what `devtab-api admin plans sync` writes, a
//! licence is issued on, and the plans' endpoints read.

use sqlx::PgExecutor;
use sqlx::types::Json;
use time::OffsetDateTime;
use uuid::Uuid;

/// The key of the catalogue's version in `app.settings`.
pub const CATALOG_KEY: &str = "plans.catalog";

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

/// The texts of a plan's card in one language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanTextRow {
    pub plan_id: String,
    pub locale: String,
    pub name: String,
    pub summary: Option<String>,
    pub includes: Vec<String>,
    pub price_note: Option<String>,
    pub badge: Option<String>,
}

/// A plan's price at a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceRow {
    pub id: Uuid,
    pub plan_id: String,
    pub provider: String,
    pub provider_price_id: String,
    pub currency: String,
    pub amount_minor: i64,
    pub active: bool,
}

/// A price to write: what the provider states for its price id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPrice {
    pub plan_id: String,
    pub provider: String,
    pub provider_price_id: String,
    pub currency: String,
    pub amount_minor: i64,
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

/// Every plan's texts, by plan and language.
pub async fn texts(db: impl PgExecutor<'_>) -> Result<Vec<PlanTextRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"select plan_id, locale, name, summary, includes as "includes: Json<Vec<String>>",
                  price_note, badge
           from app.plan_texts order by plan_id, locale"#,
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| PlanTextRow {
            plan_id: r.plan_id,
            locale: r.locale,
            name: r.name,
            summary: r.summary,
            includes: r.includes.0,
            price_note: r.price_note,
            badge: r.badge,
        })
        .collect())
}

/// Creates or replaces a plan's texts in one language.
pub async fn upsert_text(db: impl PgExecutor<'_>, text: &PlanTextRow) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "insert into app.plan_texts (plan_id, locale, name, summary, includes, price_note, badge)
         values ($1, $2, $3, $4, $5, $6, $7)
         on conflict (plan_id, locale) do update set
           name = excluded.name, summary = excluded.summary, includes = excluded.includes,
           price_note = excluded.price_note, badge = excluded.badge",
        text.plan_id,
        text.locale,
        text.name,
        text.summary,
        Json(&text.includes) as _,
        text.price_note,
        text.badge,
    )
    .execute(db)
    .await
    .map(|_| ())
}

pub async fn delete_text(
    db: impl PgExecutor<'_>,
    plan_id: &str,
    locale: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "delete from app.plan_texts where plan_id = $1 and locale = $2",
        plan_id,
        locale,
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// Every price, active or not.
pub async fn prices(db: impl PgExecutor<'_>) -> Result<Vec<PriceRow>, sqlx::Error> {
    sqlx::query_as!(
        PriceRow,
        r#"select id, plan_id, provider, provider_price_id, currency::text as "currency!",
                  amount_minor, active
           from app.plan_prices order by plan_id, provider, currency, created_at"#,
    )
    .fetch_all(db)
    .await
}

/// The active prices of one provider.
pub async fn active_prices(
    db: impl PgExecutor<'_>,
    provider: &str,
) -> Result<Vec<PriceRow>, sqlx::Error> {
    sqlx::query_as!(
        PriceRow,
        r#"select id, plan_id, provider, provider_price_id, currency::text as "currency!",
                  amount_minor, active
           from app.plan_prices where provider = $1 and active
           order by plan_id, currency"#,
        provider,
    )
    .fetch_all(db)
    .await
}

pub async fn insert_price(db: impl PgExecutor<'_>, price: &NewPrice) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar!(
        "insert into app.plan_prices (plan_id, provider, provider_price_id, currency, amount_minor)
         values ($1, $2, $3, $4, $5)
         returning id",
        price.plan_id,
        price.provider,
        price.provider_price_id,
        price.currency as _,
        price.amount_minor,
    )
    .fetch_one(db)
    .await
}

/// Makes a price active with what the provider states now.
pub async fn update_price(
    db: impl PgExecutor<'_>,
    id: Uuid,
    currency: &str,
    amount_minor: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.plan_prices set currency = $2, amount_minor = $3, active = true where id = $1",
        id,
        currency as _,
        amount_minor,
    )
    .execute(db)
    .await
    .map(|_| ())
}

pub async fn deactivate_price(db: impl PgExecutor<'_>, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "update app.plan_prices set active = false where id = $1",
        id
    )
    .execute(db)
    .await
    .map(|_| ())
}

/// The catalogue's version, if a sync ever wrote one.
pub async fn catalog_version(db: impl PgExecutor<'_>) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar!("select value from app.settings where key = $1", CATALOG_KEY)
        .fetch_optional(db)
        .await
}

pub async fn set_catalog_version(
    db: impl PgExecutor<'_>,
    version: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "insert into app.settings (key, value) values ($1, $2)
         on conflict (key) do update set value = excluded.value",
        CATALOG_KEY,
        version,
    )
    .execute(db)
    .await
    .map(|_| ())
}
