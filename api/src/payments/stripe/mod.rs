//! Stripe behind [`PaymentProvider`]: prices by id or lookup key (for
//! `admin plans sync`), the start of a subscription checkout (hosted page or
//! embedded form), a customer portal session and the check of a webhook's
//! signature ([`webhook`]).
//!
//! Everything that differs between deployments is a setting
//! ([`StripeSettings`], read from `DEVTAB_PAYMENTS_*` and `DEVTAB_STRIPE_*`):
//! the mode, the secrets' files, Managed Payments or the operator's own
//! account, tax, the checkout's form, the API's address and version, the
//! timeouts. The code holds no account, price, key or address of anyone.

pub mod client;
pub mod keys;
pub mod subscriptions;
pub mod webhook;

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use uuid::Uuid;

use self::client::Client;
use self::keys::ApiKey;
use super::{
    Billing, FoundPrice, PaymentProvider, PriceKey, PriceSnapshot, ProviderError, ProviderFuture,
};

/// The provider's name in `DEVTAB_PAYMENTS` and `config/plans.toml`.
pub const ID: &str = "stripe";

/// Stripe's API, unless `DEVTAB_STRIPE_API_URL` names a stand-in.
pub const DEFAULT_API_URL: &str = "https://api.stripe.com";

/// The API version sent unless `DEVTAB_STRIPE_API_VERSION` names another; the
/// webhook destination is created with the same one.
pub const DEFAULT_API_VERSION: &str = "2026-11-18.endive";

/// The oldest version accepted: the first of the Endive release, whose
/// shapes the adapter reads (`ui_mode=embedded_page`, the subscription's
/// period on its item, the invoice's subscription under `parent`).
pub const OLDEST_API_VERSION: &str = "2026-09-30.endive";

/// Which of the account's two worlds the keys belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripeMode {
    /// A sandbox: test keys, test cards, no money.
    Sandbox,
    /// Live mode: real payments.
    Live,
}

impl StripeMode {
    /// `DEVTAB_PAYMENTS_ENV`'s value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sandbox" => Some(Self::Sandbox),
            "live" => Some(Self::Live),
            _ => None,
        }
    }

    /// Whether an event's `livemode` belongs to this mode.
    pub fn is_live(self) -> bool {
        self == Self::Live
    }
}

impl fmt::Display for StripeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sandbox => "sandbox",
            Self::Live => "live",
        })
    }
}

/// How the buyer sees the checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutUi {
    /// The site sends the buyer to Stripe's page and back.
    Hosted,
    /// The site shows Stripe's form in its own page (Stripe.js with the
    /// publishable key and the session's client secret).
    Embedded,
}

impl CheckoutUi {
    /// `DEVTAB_STRIPE_CHECKOUT_UI`'s value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "hosted" => Some(Self::Hosted),
            "embedded" => Some(Self::Embedded),
            _ => None,
        }
    }
}

/// Stripe's settings, all from the environment (`config.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripeSettings {
    /// `DEVTAB_PAYMENTS_ENV`.
    pub mode: StripeMode,
    /// `DEVTAB_PAYMENTS_API_KEY_FILE`: the restricted key.
    pub api_key_file: PathBuf,
    /// `DEVTAB_PAYMENTS_WEBHOOK_SECRET_FILE`: the webhook destination's
    /// secret; `serve` needs it, the commands do not.
    pub webhook_secret_file: Option<PathBuf>,
    /// `DEVTAB_STRIPE_MANAGED_PAYMENTS`: Stripe sells as merchant of record.
    pub managed_payments: bool,
    /// `DEVTAB_STRIPE_AUTOMATIC_TAX`: Stripe Tax on the operator's own
    /// account; never with Managed Payments.
    pub automatic_tax: bool,
    /// `DEVTAB_STRIPE_CHECKOUT_UI`.
    pub checkout_ui: CheckoutUi,
    /// `DEVTAB_STRIPE_PUBLISHABLE_KEY`: public by design; the embedded form
    /// needs it.
    pub publishable_key: Option<String>,
    /// `DEVTAB_STRIPE_API_URL`.
    pub api_url: String,
    /// `DEVTAB_STRIPE_API_VERSION`.
    pub api_version: String,
    /// `DEVTAB_STRIPE_TIMEOUT`: seconds one attempt of a request may take.
    pub timeout_s: u32,
    /// `DEVTAB_STRIPE_WEBHOOK_TOLERANCE`: seconds a webhook's signature
    /// stays valid after Stripe made it.
    pub webhook_tolerance_s: u32,
}

/// Whether `text` is an API version as Stripe writes it, `YYYY-MM-DD.name`,
/// and not older than [`OLDEST_API_VERSION`].
pub fn is_supported_api_version(text: &str) -> bool {
    let Some((date, name)) = text.split_once('.') else {
        return false;
    };
    let date_ok = date.len() == 10
        && date.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        });
    let name_ok = !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase());
    date_ok && name_ok && text >= OLDEST_API_VERSION
}

/// What starting a checkout needs from the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRequest {
    /// Our checkout's id: the session's `client_reference_id`, its and its
    /// subscription's metadata, and its idempotency key.
    pub checkout_id: Uuid,
    /// The provider's price id, from the stored catalogue.
    pub price_id: String,
    /// How many: a team's members, 1 for a person.
    pub quantity: u32,
    /// A team: the buyer may change the number on the checkout page, from
    /// this smallest team up to 1000.
    pub adjustable_from: Option<u32>,
    /// The buyer's Stripe customer, when the account already has one.
    pub customer: Option<String>,
    /// The checkout's language (`en`, `ru`, …); Stripe's own choice without.
    pub locale: Option<String>,
    /// Where Stripe sends the buyer after paying (hosted) or after the form
    /// is done (embedded).
    pub success_url: String,
    /// Where Stripe's back button goes (hosted only).
    pub cancel_url: String,
}

/// How the site continues a started checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutStep {
    /// Send the buyer to this Stripe address.
    Redirect { url: String },
    /// Mount Stripe's form with this secret, the publishable key and Stripe.js
    /// of the session's API version.
    Embedded {
        client_secret: String,
        publishable_key: String,
        api_version: String,
    },
}

/// A started checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutStart {
    /// `cs_…`.
    pub provider_checkout_id: String,
    pub step: CheckoutStep,
}

/// The metadata key that ties Stripe's objects to our checkout.
pub const CHECKOUT_METADATA_KEY: &str = "devtab_checkout";

/// Stripe as a payment provider.
#[derive(Debug, Clone)]
pub struct StripeProvider {
    client: Client,
    settings: StripeSettings,
}

impl StripeProvider {
    /// Reads the API key from its file and builds the client.
    pub fn connect(settings: StripeSettings) -> anyhow::Result<Self> {
        let key = ApiKey::read(&settings.api_key_file, settings.mode)?;
        Self::with_key(settings, key)
    }

    /// The provider with a key already read.
    pub fn with_key(settings: StripeSettings, key: ApiKey) -> anyhow::Result<Self> {
        let client = Client::new(
            &settings.api_url,
            key,
            &settings.api_version,
            Duration::from_secs(u64::from(settings.timeout_s)),
        )?;
        Ok(Self { client, settings })
    }

    /// Other pauses between retries (tests use none).
    #[must_use]
    pub fn with_retry_delays(mut self, delays: Vec<Duration>) -> Self {
        self.client = self.client.with_retry_delays(delays);
        self
    }

    pub fn settings(&self) -> &StripeSettings {
        &self.settings
    }

    /// The form of `POST /v1/checkout/sessions` for `request`.
    pub fn checkout_form(&self, request: &CheckoutRequest) -> Vec<(String, String)> {
        let checkout = request.checkout_id.to_string();
        let mut form: Vec<(String, String)> = vec![
            ("mode".into(), "subscription".into()),
            ("line_items[0][price]".into(), request.price_id.clone()),
            (
                "line_items[0][quantity]".into(),
                request.quantity.to_string(),
            ),
            ("client_reference_id".into(), checkout.clone()),
            (
                format!("metadata[{CHECKOUT_METADATA_KEY}]"),
                checkout.clone(),
            ),
            (
                format!("subscription_data[metadata][{CHECKOUT_METADATA_KEY}]"),
                checkout,
            ),
        ];
        if let Some(min) = request.adjustable_from {
            form.push((
                "line_items[0][adjustable_quantity][enabled]".into(),
                "true".into(),
            ));
            form.push((
                "line_items[0][adjustable_quantity][minimum]".into(),
                min.to_string(),
            ));
            form.push((
                "line_items[0][adjustable_quantity][maximum]".into(),
                "1000".into(),
            ));
        }
        match self.settings.checkout_ui {
            CheckoutUi::Hosted => {
                form.push(("success_url".into(), request.success_url.clone()));
                form.push(("cancel_url".into(), request.cancel_url.clone()));
            }
            CheckoutUi::Embedded => {
                form.push(("ui_mode".into(), "embedded_page".into()));
                form.push(("return_url".into(), request.success_url.clone()));
            }
        }
        if let Some(customer) = &request.customer {
            form.push(("customer".into(), customer.clone()));
        }
        if let Some(locale) = &request.locale {
            form.push(("locale".into(), locale.clone()));
        }
        if self.settings.managed_payments {
            form.push(("managed_payments[enabled]".into(), "true".into()));
        } else if self.settings.automatic_tax {
            form.push(("automatic_tax[enabled]".into(), "true".into()));
        }
        form
    }

    /// Starts a subscription checkout for one price.
    pub async fn create_checkout(
        &self,
        request: &CheckoutRequest,
    ) -> Result<CheckoutStart, ProviderError> {
        let form = self.checkout_form(request);
        let key = format!("checkout-{}", request.checkout_id);
        let session = self
            .client
            .post("/v1/checkout/sessions", &form, &key)
            .await?;
        let id = text(&session, "id")?;
        let step = match self.settings.checkout_ui {
            CheckoutUi::Hosted => CheckoutStep::Redirect {
                url: text(&session, "url")?,
            },
            CheckoutUi::Embedded => CheckoutStep::Embedded {
                client_secret: text(&session, "client_secret")?,
                publishable_key: self.settings.publishable_key.clone().ok_or_else(|| {
                    ProviderError::Refused(
                        "the embedded checkout needs DEVTAB_STRIPE_PUBLISHABLE_KEY".to_owned(),
                    )
                })?,
                api_version: self.client.version().to_owned(),
            },
        };
        Ok(CheckoutStart {
            provider_checkout_id: id,
            step,
        })
    }

    /// A customer portal session for `customer`, which returns to
    /// `return_url`; it expires soon, so it is made when the buyer asks.
    pub async fn portal_url(
        &self,
        customer: &str,
        return_url: &str,
        request_id: Uuid,
    ) -> Result<String, ProviderError> {
        let form = vec![
            ("customer".to_owned(), customer.to_owned()),
            ("return_url".to_owned(), return_url.to_owned()),
        ];
        let key = format!("portal-{request_id}");
        let session = self
            .client
            .post("/v1/billing_portal/sessions", &form, &key)
            .await?;
        text(&session, "url")
    }

    async fn price_by_id(&self, id: &str) -> Result<FoundPrice, ProviderError> {
        if !is_object_id(id) {
            return Err(ProviderError::NotFound(format!("price `{id}`")));
        }
        let price = self.client.get(&format!("/v1/prices/{id}"), &[]).await?;
        read_price(&price)
    }

    async fn price_by_lookup_key(&self, key: &str) -> Result<FoundPrice, ProviderError> {
        let list = self
            .client
            .get(
                "/v1/prices",
                &[("lookup_keys[]", key), ("active", "true"), ("limit", "2")],
            )
            .await?;
        match list
            .get("data")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
        {
            Some([price]) => read_price(price),
            Some([]) => Err(ProviderError::NotFound(format!(
                "active price with lookup key `{key}`"
            ))),
            Some(_) => Err(ProviderError::Refused(format!(
                "more than one active price has lookup key `{key}`"
            ))),
            None => Err(ProviderError::Unavailable(
                "Stripe's list of prices has no `data`".to_owned(),
            )),
        }
    }
}

impl PaymentProvider for StripeProvider {
    fn id(&self) -> &'static str {
        ID
    }

    fn price<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PriceSnapshot> {
        Box::pin(async move { Ok(self.price_by_id(id).await?.snapshot) })
    }

    fn resolve<'a>(&'a self, key: &'a PriceKey) -> ProviderFuture<'a, FoundPrice> {
        Box::pin(async move {
            match key {
                PriceKey::Id(id) => self.price_by_id(id).await,
                PriceKey::LookupKey(key) => self.price_by_lookup_key(key).await,
            }
        })
    }
}

/// A Stripe object id: letters, digits and `_`, so that it is safe in a path.
fn is_object_id(id: &str) -> bool {
    (1..=255).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn text(object: &Value, field: &str) -> Result<String, ProviderError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::Unavailable(format!("Stripe's answer has no `{field}`")))
}

/// A Price object: its id, a fixed amount per unit, the currency and how
/// often it bills (`month`; `3 month` when every three months).
fn read_price(price: &Value) -> Result<FoundPrice, ProviderError> {
    let id = text(price, "id")?;
    if price.get("active").and_then(Value::as_bool) == Some(false) {
        return Err(ProviderError::NotFound(format!("active price {id}")));
    }
    let amount_minor = price
        .get("unit_amount")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            ProviderError::Refused(format!(
                "price {id} has no fixed unit amount (tiered or customer-chosen prices are not \
                 plans)"
            ))
        })?;
    let currency = text(price, "currency")?.to_ascii_uppercase();
    let billing = match price.get("recurring").filter(|r| !r.is_null()) {
        None => Billing::OneTime,
        Some(recurring) => {
            let unit = recurring
                .get("interval")
                .and_then(Value::as_str)
                .unwrap_or("?");
            Billing::Every(
                match recurring.get("interval_count").and_then(Value::as_i64) {
                    None | Some(1) => unit.to_owned(),
                    Some(count) => format!("{count} {unit}"),
                },
            )
        }
    };
    Ok(FoundPrice {
        id,
        snapshot: PriceSnapshot {
            currency,
            amount_minor,
        },
        billing,
    })
}

#[cfg(test)]
mod tests;
