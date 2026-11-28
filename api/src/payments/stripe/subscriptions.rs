//! What Stripe's events mean to us ([`signals`], without the network), a
//! subscription's state now, and which subscription a refunded charge or a
//! closed dispute belongs to: Stripe as an [`EventSource`].
//!
//! Since the API version `2025-03-31.basil` a subscription's period lives on
//! its item and an invoice names its subscription under `parent`; a charge
//! no longer names its invoice, which is found through the invoice payment
//! of its payment intent.

use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{CHECKOUT_METADATA_KEY, StripeProvider, is_object_id, text};
use crate::payments::{
    EventSource, PaymentSnapshot, ProviderError, ProviderFuture, Signal, SubscriptionSnapshot,
    SubscriptionStatus,
};

/// What an event of `kind` about `object` (its `data.object`) means to us.
pub fn signals(kind: &str, object: &Value) -> Vec<Signal> {
    let str_field = |name: &str| object.get(name).and_then(Value::as_str).map(str::to_owned);
    let signal = match kind {
        "checkout.session.completed" => checkout_completed(object),
        "checkout.session.async_payment_succeeded" | "checkout.session.async_payment_failed" => {
            ours(object)
                .and(str_field("subscription"))
                .map(|subscription| Signal::SubscriptionChanged { subscription })
        }
        "customer.subscription.created"
        | "customer.subscription.updated"
        | "customer.subscription.deleted"
        | "customer.subscription.paused"
        | "customer.subscription.resumed" => {
            str_field("id").map(|subscription| Signal::SubscriptionChanged { subscription })
        }
        "invoice.paid" => {
            invoice_subscription(object).map(|subscription| Signal::InvoicePaid { subscription })
        }
        "invoice.payment_failed" | "invoice.payment_action_required" => {
            invoice_subscription(object).map(|subscription| Signal::PaymentFailed { subscription })
        }
        "charge.refunded" => str_field("id").map(|charge| Signal::Refunded { charge }),
        "charge.dispute.closed" => str_field("id").map(|dispute| Signal::DisputeClosed { dispute }),
        _ => None,
    };
    vec![signal.unwrap_or(Signal::Ignored)]
}

/// Our checkout id, when the session is ours (a subscription checkout with
/// our id as its client reference).
fn ours(session: &Value) -> Option<Uuid> {
    if session.get("mode").and_then(Value::as_str) != Some("subscription") {
        return None;
    }
    session
        .get("client_reference_id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
}

fn checkout_completed(session: &Value) -> Option<Signal> {
    let checkout = ours(session)?;
    let id_of = |name: &str| {
        session.get(name).and_then(|value| match value {
            Value::String(id) => Some(id.clone()),
            // An expanded object.
            Value::Object(_) => value.get("id").and_then(Value::as_str).map(str::to_owned),
            _ => None,
        })
    };
    Some(Signal::CheckoutCompleted {
        checkout,
        customer: id_of("customer")?,
        subscription: id_of("subscription")?,
    })
}

/// `parent.subscription_details.subscription` of an invoice whose parent is
/// a subscription.
fn invoice_subscription(invoice: &Value) -> Option<String> {
    let parent = invoice.get("parent")?;
    if parent.get("type").and_then(Value::as_str) != Some("subscription_details") {
        return None;
    }
    parent
        .get("subscription_details")?
        .get("subscription")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

impl StripeProvider {
    /// The subscription's state now, with its latest invoice.
    pub async fn fetch_subscription(
        &self,
        id: &str,
    ) -> Result<SubscriptionSnapshot, ProviderError> {
        if !is_object_id(id) {
            return Err(ProviderError::NotFound(format!("subscription `{id}`")));
        }
        let subscription = self
            .client
            .get(
                &format!("/v1/subscriptions/{id}"),
                &[("expand[]", "latest_invoice")],
            )
            .await?;
        read_subscription(&subscription)
    }

    /// The payment behind a refunded charge.
    pub async fn fetch_charge(&self, id: &str) -> Result<PaymentSnapshot, ProviderError> {
        if !is_object_id(id) {
            return Err(ProviderError::NotFound(format!("charge `{id}`")));
        }
        let charge = self.client.get(&format!("/v1/charges/{id}"), &[]).await?;
        let refunded_fully = charge.get("refunded").and_then(Value::as_bool) == Some(true);
        Ok(PaymentSnapshot {
            subscription: self.subscription_of_payment(&charge).await?,
            refunded_fully,
            dispute_lost: false,
        })
    }

    /// The payment behind a closed dispute.
    pub async fn fetch_dispute(&self, id: &str) -> Result<PaymentSnapshot, ProviderError> {
        if !is_object_id(id) {
            return Err(ProviderError::NotFound(format!("dispute `{id}`")));
        }
        let dispute = self.client.get(&format!("/v1/disputes/{id}"), &[]).await?;
        let dispute_lost = dispute.get("status").and_then(Value::as_str) == Some("lost");
        Ok(PaymentSnapshot {
            subscription: self.subscription_of_payment(&dispute).await?,
            refunded_fully: false,
            dispute_lost,
        })
    }

    /// The address the buyer typed in a Checkout Session: its
    /// `customer_details.email` (Stripe collects one in every subscription
    /// checkout without a customer), else `customer_email`. A Checkout
    /// Session is readable with the key's write permission on Checkout
    /// Sessions; no permission on Customers is needed.
    pub async fn fetch_checkout_email(&self, id: &str) -> Result<Option<String>, ProviderError> {
        if !is_object_id(id) {
            return Err(ProviderError::NotFound(format!("checkout session `{id}`")));
        }
        let session = self
            .client
            .get(&format!("/v1/checkout/sessions/{id}"), &[])
            .await?;
        Ok(checkout_email(&session))
    }

    /// The subscription a charge or dispute paid for: its payment intent →
    /// the paid invoice payment → the invoice's subscription. `None` for a
    /// payment no subscription invoice holds.
    async fn subscription_of_payment(
        &self,
        object: &Value,
    ) -> Result<Option<String>, ProviderError> {
        let Some(intent) = object.get("payment_intent").and_then(Value::as_str) else {
            return Ok(None);
        };
        let payments = self
            .client
            .get(
                "/v1/invoice_payments",
                &[
                    ("payment[type]", "payment_intent"),
                    ("payment[payment_intent]", intent),
                    ("status", "paid"),
                ],
            )
            .await?;
        let invoice = payments
            .get("data")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(|payment| payment.get("invoice"))
            .and_then(Value::as_str);
        let Some(invoice) = invoice.filter(|id| is_object_id(id)) else {
            return Ok(None);
        };
        let invoice = self
            .client
            .get(&format!("/v1/invoices/{invoice}"), &[])
            .await?;
        Ok(invoice_subscription(&invoice))
    }
}

impl EventSource for StripeProvider {
    fn id(&self) -> &'static str {
        super::ID
    }

    fn signals(&self, kind: &str, object: &Value) -> Vec<Signal> {
        signals(kind, object)
    }

    fn subscription<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, SubscriptionSnapshot> {
        Box::pin(self.fetch_subscription(id))
    }

    fn charge<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PaymentSnapshot> {
        Box::pin(self.fetch_charge(id))
    }

    fn dispute<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PaymentSnapshot> {
        Box::pin(self.fetch_dispute(id))
    }

    fn checkout_email<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, Option<String>> {
        Box::pin(self.fetch_checkout_email(id))
    }
}

/// The buyer's address in a Checkout Session.
pub fn checkout_email(session: &Value) -> Option<String> {
    session
        .get("customer_details")
        .and_then(|details| details.get("email"))
        .and_then(Value::as_str)
        .or_else(|| session.get("customer_email").and_then(Value::as_str))
        .filter(|email| !email.trim().is_empty())
        .map(str::to_owned)
}

/// A Subscription object with `latest_invoice` expanded.
pub fn read_subscription(subscription: &Value) -> Result<SubscriptionSnapshot, ProviderError> {
    let id = text(subscription, "id")?;
    let status = match text(subscription, "status")?.as_str() {
        // No trials are offered; a trialing subscription would be running.
        "active" | "trialing" => SubscriptionStatus::Active,
        "past_due" => SubscriptionStatus::PastDue,
        "incomplete" => SubscriptionStatus::Incomplete,
        "paused" => SubscriptionStatus::Paused,
        // canceled, unpaid (retries over), incomplete_expired.
        _ => SubscriptionStatus::Ended,
    };
    let item = subscription
        .get("items")
        .and_then(|items| items.get("data"))
        .and_then(Value::as_array)
        .and_then(|data| match data.as_slice() {
            [item] => Some(item),
            _ => None,
        })
        .ok_or_else(|| {
            ProviderError::Refused(format!("subscription {id} does not have exactly one item"))
        })?;
    let price_id = item
        .get("price")
        .and_then(|price| price.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::Unavailable(format!("subscription {id}: no price")))?;
    let current_period_end = item
        .get("current_period_end")
        .and_then(Value::as_i64)
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
        .ok_or_else(|| {
            ProviderError::Unavailable(format!("subscription {id}: no current_period_end"))
        })?;
    let quantity = item
        .get("quantity")
        .and_then(Value::as_u64)
        .and_then(|q| u32::try_from(q).ok())
        .filter(|q| *q > 0)
        .unwrap_or(1);
    let cancel_at_period_end = subscription
        .get("cancel_at_period_end")
        .and_then(Value::as_bool)
        == Some(true)
        || subscription
            .get("cancel_at")
            .is_some_and(|at| !at.is_null());
    let customer = match subscription.get("customer") {
        Some(Value::String(id)) => id.clone(),
        Some(object @ Value::Object(_)) => text(object, "id")?,
        _ => {
            return Err(ProviderError::Unavailable(format!(
                "subscription {id}: no customer"
            )));
        }
    };
    let period_paid = subscription
        .get("latest_invoice")
        .and_then(|invoice| invoice.get("status"))
        .and_then(Value::as_str)
        == Some("paid");
    let checkout = subscription
        .get("metadata")
        .and_then(|metadata| metadata.get(CHECKOUT_METADATA_KEY))
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    Ok(SubscriptionSnapshot {
        id,
        status,
        price_id,
        quantity,
        current_period_end,
        cancel_at_period_end,
        customer,
        period_paid,
        checkout,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const CHECKOUT: &str = "00000000-0000-0000-0000-000000000001";

    fn one(kind: &str, object: &Value) -> Signal {
        let mut list = signals(kind, object);
        assert_eq!(list.len(), 1);
        list.remove(0)
    }

    #[test]
    fn a_completed_checkout_of_ours_is_read_and_others_are_ignored() {
        let session = json!({
            "id": "cs_1", "mode": "subscription", "client_reference_id": CHECKOUT,
            "customer": "cus_1", "subscription": {"id": "sub_1"},
        });
        assert_eq!(
            one("checkout.session.completed", &session),
            Signal::CheckoutCompleted {
                checkout: Uuid::parse_str(CHECKOUT).unwrap(),
                customer: "cus_1".to_owned(),
                subscription: "sub_1".to_owned(),
            }
        );
        for other in [
            json!({"mode": "payment", "client_reference_id": CHECKOUT, "customer": "c", "subscription": "s"}),
            json!({"mode": "subscription", "client_reference_id": "not-ours", "customer": "c", "subscription": "s"}),
            json!({"mode": "subscription", "customer": "c", "subscription": "s"}),
        ] {
            assert_eq!(one("checkout.session.completed", &other), Signal::Ignored);
        }
    }

    #[test]
    fn subscription_invoice_and_payment_events() {
        let sub = |subscription: &str| Signal::SubscriptionChanged {
            subscription: subscription.to_owned(),
        };
        assert_eq!(
            one("customer.subscription.updated", &json!({"id": "sub_1"})),
            sub("sub_1")
        );
        assert_eq!(
            one(
                "checkout.session.async_payment_succeeded",
                &json!({"mode": "subscription", "client_reference_id": CHECKOUT, "subscription": "sub_2"})
            ),
            sub("sub_2")
        );
        let invoice = json!({"id": "in_1", "parent": {"type": "subscription_details",
                             "subscription_details": {"subscription": "sub_3"}}});
        assert_eq!(
            one("invoice.paid", &invoice),
            Signal::InvoicePaid {
                subscription: "sub_3".to_owned()
            }
        );
        assert_eq!(
            one("invoice.payment_failed", &invoice),
            Signal::PaymentFailed {
                subscription: "sub_3".to_owned()
            }
        );
        let one_off = json!({"id": "in_2", "parent": {"type": "quote_details"}});
        assert_eq!(one("invoice.paid", &one_off), Signal::Ignored);
        assert_eq!(
            one("charge.refunded", &json!({"id": "ch_1"})),
            Signal::Refunded {
                charge: "ch_1".to_owned()
            }
        );
        assert_eq!(
            one("charge.dispute.closed", &json!({"id": "dp_1"})),
            Signal::DisputeClosed {
                dispute: "dp_1".to_owned()
            }
        );
        for ignored in [
            "charge.dispute.created",
            "checkout.session.expired",
            "customer.updated",
        ] {
            assert_eq!(
                one(ignored, &json!({"id": "x"})),
                Signal::Ignored,
                "{ignored}"
            );
        }
    }

    #[test]
    fn the_buyer_s_address_is_the_one_typed_in_the_checkout() {
        let session = json!({"customer_details": {"email": "Buyer@Example.invalid"},
                             "customer_email": "other@example.invalid"});
        assert_eq!(
            checkout_email(&session).as_deref(),
            Some("Buyer@Example.invalid")
        );
        let prefilled = json!({"customer_details": null, "customer_email": "a@example.invalid"});
        assert_eq!(
            checkout_email(&prefilled).as_deref(),
            Some("a@example.invalid")
        );
        assert_eq!(
            checkout_email(&json!({"customer_details": {"email": ""}})),
            None
        );
    }

    fn subscription(status: &str, invoice: &str) -> Value {
        json!({
            "id": "sub_1", "status": status, "customer": "cus_1",
            "cancel_at_period_end": false, "cancel_at": null,
            "metadata": {"devtab_checkout": CHECKOUT},
            "items": {"data": [{"price": {"id": "price_month"}, "current_period_end": 1_796_083_200}]},
            "latest_invoice": {"id": "in_1", "status": invoice},
        })
    }

    #[test]
    fn a_subscription_s_state_is_read_from_its_item_and_latest_invoice() {
        let snapshot = read_subscription(&subscription("active", "paid")).unwrap();
        assert_eq!(snapshot.status, SubscriptionStatus::Active);
        assert_eq!(snapshot.price_id, "price_month");
        assert_eq!(snapshot.current_period_end.unix_timestamp(), 1_796_083_200);
        assert!(snapshot.period_paid);
        assert!(!snapshot.cancel_at_period_end);
        assert_eq!(snapshot.customer, "cus_1");
        assert_eq!(snapshot.checkout, Uuid::parse_str(CHECKOUT).ok());
        assert!(
            !read_subscription(&subscription("active", "open"))
                .unwrap()
                .period_paid
        );
        for (status, want) in [
            ("past_due", SubscriptionStatus::PastDue),
            ("incomplete", SubscriptionStatus::Incomplete),
            ("paused", SubscriptionStatus::Paused),
            ("canceled", SubscriptionStatus::Ended),
            ("unpaid", SubscriptionStatus::Ended),
            ("incomplete_expired", SubscriptionStatus::Ended),
        ] {
            assert_eq!(
                read_subscription(&subscription(status, "open"))
                    .unwrap()
                    .status,
                want
            );
        }
        let mut cancel_at = subscription("active", "paid");
        cancel_at["cancel_at"] = json!(1_796_083_200);
        assert!(read_subscription(&cancel_at).unwrap().cancel_at_period_end);
        let mut two_items = subscription("active", "paid");
        two_items["items"]["data"] = json!([{"price": {"id": "a"}}, {"price": {"id": "b"}}]);
        assert!(matches!(
            read_subscription(&two_items),
            Err(ProviderError::Refused(_))
        ));
    }
}
