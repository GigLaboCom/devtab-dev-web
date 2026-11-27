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
pub mod process;
pub mod reconcile;
pub mod stripe;

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use time::OffsetDateTime;
use uuid::Uuid;

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

/// A step of a volume-tiered price: up to this many (none: any number
/// above the previous step), each of them costs this.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tier {
    pub up_to: Option<u32>,
    pub amount_minor: i64,
}

/// A price found by its key: the provider's id of it, its amount and how
/// often it bills; a volume-tiered price has its steps (and the first step's
/// amount as its amount).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundPrice {
    pub id: String,
    pub snapshot: PriceSnapshot,
    pub billing: Billing,
    pub tiers: Vec<Tier>,
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

/// A subscription's state at its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionStatus {
    /// Running; its period is paid or its renewal is being charged.
    Active,
    /// A renewal could not be charged and the provider is retrying.
    PastDue,
    /// The first payment has not gone through: no licence yet.
    Incomplete,
    /// Over: cancelled, unpaid after the retries, or never paid.
    Ended,
    /// Paused by the provider or the buyer.
    Paused,
}

/// A subscription as its provider states it now, read when an event about it
/// arrives (events come out of order and repeat; the state does not).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionSnapshot {
    pub id: String,
    pub status: SubscriptionStatus,
    /// The provider's id of the price it bills.
    pub price_id: String,
    /// How many of the price: a team's members, 1 for a person.
    pub quantity: u32,
    pub current_period_end: OffsetDateTime,
    /// It ends at the end of the period.
    pub cancel_at_period_end: bool,
    pub customer: String,
    /// The invoice of the current period is paid; until it is, the period's
    /// end is not "paid until".
    pub period_paid: bool,
    /// Our checkout, from the subscription's metadata.
    pub checkout: Option<Uuid>,
}

/// What an event means to us, read from it without the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// A checkout of ours completed: its subscription may be paid.
    CheckoutCompleted {
        checkout: Uuid,
        customer: String,
        subscription: String,
    },
    /// Something about the subscription changed: read its state.
    SubscriptionChanged { subscription: String },
    /// Money was taken for the subscription.
    InvoicePaid { subscription: String },
    /// A charge for the subscription failed.
    PaymentFailed { subscription: String },
    /// A payment was refunded, in part or in full.
    Refunded { charge: String },
    /// A dispute over a payment is closed, won or lost.
    DisputeClosed { dispute: String },
    /// Nothing for us.
    Ignored,
}

impl Signal {
    /// The subscription whose state this signal asks to read.
    pub fn subscription(&self) -> Option<&str> {
        match self {
            Self::CheckoutCompleted { subscription, .. }
            | Self::SubscriptionChanged { subscription }
            | Self::InvoicePaid { subscription }
            | Self::PaymentFailed { subscription } => Some(subscription),
            Self::Refunded { .. } | Self::DisputeClosed { .. } | Self::Ignored => None,
        }
    }
}

/// The real provider `serve` takes payments with, and its webhook secret.
#[derive(Debug)]
pub struct Payments {
    pub stripe: stripe::StripeProvider,
    pub webhook_secret: stripe::keys::WebhookSecret,
}

/// What became of a payment: the subscription it paid for, whether it was
/// refunded in full, whether a dispute over it was lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentSnapshot {
    pub subscription: Option<String>,
    pub refunded_fully: bool,
    pub dispute_lost: bool,
}

/// What processing a provider's events needs from it: what an event means,
/// a subscription's state now, and what became of a payment.
pub trait EventSource: Send + Sync + fmt::Debug {
    /// The provider's name, as the licences and events record it.
    fn id(&self) -> &'static str;

    /// What an event of `kind` about `object` means to us; no network.
    fn signals(&self, kind: &str, object: &serde_json::Value) -> Vec<Signal>;

    fn subscription<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, SubscriptionSnapshot>;

    /// The payment behind a refunded charge.
    fn charge<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PaymentSnapshot>;

    /// The payment behind a closed dispute.
    fn dispute<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PaymentSnapshot>;
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
                    tiers: Vec::new(),
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
