//! Database access: the Postgres pool and the queries, one module per table.
//! The schema comes from `supabase/migrations/`; queries are `sqlx::query!`,
//! checked against it at build time (offline data in `.sqlx/`). They name
//! every table with its schema, `app.`, so that they do not depend on the
//! search path.

pub mod audit;
pub mod checkouts;
pub mod customers;
pub mod events;
pub mod installations;
pub mod jobs;
pub mod licences;
pub mod members;
pub mod plans;
pub mod reveals;
pub mod users;

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor as _, PgPool};

pub const PING_TIMEOUT: Duration = Duration::from_secs(2);

/// Handle to the database, cheap to clone.
#[derive(Debug, Clone)]
pub struct Store {
    pool: PgPool,
}

impl Store {
    /// Creates a lazily connecting pool: the server starts even while the
    /// database is down, and `/api/readyz` reports it.
    pub fn connect_lazy(url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(PING_TIMEOUT)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    conn.execute("set search_path = app").await?;
                    Ok(())
                })
            })
            .connect_lazy(url)?;
        Ok(Self { pool })
    }

    /// A store over a pool made elsewhere (tests that act as the API role).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Round-trips `select 1` within [`PING_TIMEOUT`].
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        match tokio::time::timeout(PING_TIMEOUT, sqlx::query("select 1").execute(&self.pool)).await
        {
            Ok(result) => result.map(|_| ()),
            Err(_) => Err(sqlx::Error::PoolTimedOut),
        }
    }
}
