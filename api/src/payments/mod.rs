//! The payment provider behind one interface, so that the rest of the API
//! never names a provider. So far it holds what the plans' catalogue needs:
//! a price's amount and currency by the provider's price id, which
//! `admin plans sync` reads and stores; the endpoints read only the stored
//! rows. Checkouts, webhooks and the rest of the interface come with the
//! purchase.
//!
//! The methods return boxed futures rather than `async fn`, so that the trait
//! stays usable as `dyn PaymentProvider` without another crate: a provider
//! that answers over HTTP, with its own price ids, fits behind it unchanged.

pub mod fake;

use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// What a provider's method returns.
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

/// A price as the provider states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceSnapshot {
    /// ISO 4217, upper case.
    pub currency: String,
    /// In the currency's minor units.
    pub amount_minor: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The provider knows no such object.
    #[error("the provider knows no {0}")]
    NotFound(String),
    /// The provider could not be asked (network, outage, limits).
    #[error("the provider could not be reached: {0}")]
    Unavailable(String),
}

/// A payment provider: `fake` (development and tests) or a real one.
pub trait PaymentProvider: Send + Sync + fmt::Debug {
    /// The provider's name, as `app.plan_prices.provider` records it.
    fn id(&self) -> &'static str;

    /// The amount and currency of the price `id` (the provider's price id).
    fn price<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PriceSnapshot>;
}

/// The providers this server knows by name, as `DEVTAB_PAYMENTS` takes them.
pub const KNOWN: [&str; 1] = [fake::ID];
