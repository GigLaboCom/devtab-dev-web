//! The payment provider behind one interface, so that the rest of the API
//! never names a provider. So far it holds what the plans' catalogue needs:
//! a price's amount and currency by the provider's price id or lookup key,
//! which `admin plans sync` reads and stores; the endpoints read only the
//! stored rows. Checkouts, webhooks and the rest of the interface come with
//! the purchase; a provider's own module may already hold them.
//!
//! The methods return boxed futures rather than `async fn`, so that the trait
//! stays usable as `dyn PaymentProvider` without another crate: a provider
//! that answers over HTTP, with its own price ids, fits behind it unchanged.

pub mod fake;
pub mod stripe;

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

/// How a plans file names a price at its provider.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PriceKey {
    /// The provider's own id of the price; it differs between a provider's
    /// test and live modes.
    Id(String),
    /// A key the operator gives the price at the provider, the same in every
    /// mode, so that one plans file serves all of them.
    LookupKey(String),
}

impl fmt::Display for PriceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => write!(f, "{id}"),
            Self::LookupKey(key) => write!(f, "lookup key {key}"),
        }
    }
}

/// How often a price bills, as far as its provider says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Billing {
    /// The provider does not say (the fake provider).
    Unknown,
    /// Once.
    OneTime,
    /// Every interval: `month`, `year`, or `3 month` for every three months.
    Every(String),
}

/// A price found by its key: the provider's id of it, its amount and how
/// often it bills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundPrice {
    pub id: String,
    pub snapshot: PriceSnapshot,
    pub billing: Billing,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The provider knows no such object.
    #[error("the provider knows no {0}")]
    NotFound(String),
    /// The provider could not be asked (network, outage, limits).
    #[error("the provider could not be reached: {0}")]
    Unavailable(String),
    /// The provider refused the request (its key, its permissions, the
    /// request itself); asking again does not help.
    #[error("the provider refused the request: {0}")]
    Refused(String),
    /// The provider has no such thing at all.
    #[error("the provider does not support {0}")]
    Unsupported(String),
}

/// A payment provider: `fake` (development and tests) or a real one.
pub trait PaymentProvider: Send + Sync + fmt::Debug {
    /// The provider's name, as `app.plan_prices.provider` records it.
    fn id(&self) -> &'static str;

    /// The amount and currency of the price `id` (the provider's price id).
    fn price<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PriceSnapshot>;

    /// The price a plans file names: by id through [`Self::price`]; a
    /// provider with lookup keys answers those too.
    fn resolve<'a>(&'a self, key: &'a PriceKey) -> ProviderFuture<'a, FoundPrice> {
        Box::pin(async move {
            match key {
                PriceKey::Id(id) => Ok(FoundPrice {
                    id: id.clone(),
                    snapshot: self.price(id).await?,
                    billing: Billing::Unknown,
                }),
                PriceKey::LookupKey(_) => Err(ProviderError::Unsupported(format!(
                    "lookup keys at {}",
                    self.id()
                ))),
            }
        })
    }
}

/// The providers this server knows by name, as `DEVTAB_PAYMENTS` takes them.
pub const KNOWN: [&str; 2] = [fake::ID, stripe::ID];
