//! `process_event`: what a stored provider event changes, in the caller's
//! transaction.
//!
//! Nothing changes a licence without a verified event, and nothing is taken
//! from the event's body but ids: for each subscription it names, the
//! provider's state now is read and [`reconcile`](super::reconcile) decides.
//! Events arrive out of order and repeat; the state does not.
//!
//! Each subscription is handled under a transaction-scoped advisory lock on
//! its provider and id, taken before anything is read, so that two events of
//! one purchase processed at once (a completed checkout and its paid
//! invoice, which arrive together in either order) cannot both find no
//! licence and issue two.
//!
//! A licence is issued only from one of our checkouts, to the person who
//! started it (never by an address in the event): the provider's customer is
//! tied to that person, the key is kept to be shown once, and the checkout is
//! completed.

use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::reconcile::{Decision, LicenceState, LicenceStatus, reconcile};
use super::{EventSource, ProviderError, Signal, SubscriptionSnapshot};
use crate::licence::key;
use crate::store::licences::{NewLicence, SubscriptionLicence};
use crate::store::{checkouts, customers, events, licences, plans, reveals};

/// How long a new key waits to be shown.
pub const REVEAL_FOR: Duration = Duration::hours(24);

/// Why processing stopped.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// Worth trying again later: the provider or the database could not be
    /// reached.
    #[error("{0}")]
    Retry(String),
    /// Trying again cannot help; someone has to look (an alarm).
    #[error("{0}")]
    Permanent(String),
}

impl From<sqlx::Error> for ProcessError {
    fn from(err: sqlx::Error) -> Self {
        Self::Retry(format!("database: {err}"))
    }
}

impl From<ProviderError> for ProcessError {
    fn from(err: ProviderError) -> Self {
        match err {
            ProviderError::Unavailable(_) => Self::Retry(err.to_string()),
            _ => Self::Permanent(err.to_string()),
        }
    }
}

/// What processing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Processed {
    /// The event changed or confirmed something.
    Processed,
    /// The event means nothing to us.
    Ignored,
    /// Processed before (a repeated job).
    AlreadyDone,
}

/// What happened to one subscription's licence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenceChange {
    Issued(Uuid),
    Updated(Uuid),
    Unchanged,
    /// No licence, and none due yet.
    Waiting,
}

/// Processes the stored event `id` in the caller's transaction.
pub async fn process_event(
    conn: &mut PgConnection,
    source: &dyn EventSource,
    id: i64,
    now: OffsetDateTime,
) -> Result<Processed, ProcessError> {
    let event = events::find_for_update(&mut *conn, id)
        .await?
        .ok_or_else(|| ProcessError::Permanent(format!("no event {id}")))?;
    if matches!(event.status.as_str(), "processed" | "ignored") {
        return Ok(Processed::AlreadyDone);
    }
    if event.provider != source.id() {
        return Err(ProcessError::Permanent(format!(
            "event {id} is from {}, this server takes {}",
            event.provider,
            source.id()
        )));
    }
    let object = event
        .payload
        .as_ref()
        .and_then(|payload| payload.get("data"))
        .and_then(|data| data.get("object"))
        .ok_or_else(|| ProcessError::Permanent(format!("event {id} has no body to process")))?;
    let mut acted = false;
    for signal in source.signals(&event.event_type, object) {
        match signal {
            Signal::Ignored => {}
            Signal::CheckoutCompleted {
                checkout,
                subscription,
                ..
            } => {
                acted = true;
                sync_subscription(conn, source, &subscription, Some(checkout), now).await?;
            }
            Signal::SubscriptionChanged { subscription }
            | Signal::InvoicePaid { subscription }
            | Signal::PaymentFailed { subscription } => {
                acted = true;
                sync_subscription(conn, source, &subscription, None, now).await?;
            }
            Signal::Refunded { charge } => {
                acted = true;
                let payment = source.charge(&charge).await?;
                if payment.refunded_fully {
                    revoke(conn, source, payment.subscription.as_deref(), now).await?;
                }
            }
            Signal::DisputeClosed { dispute } => {
                acted = true;
                let payment = source.dispute(&dispute).await?;
                if payment.dispute_lost {
                    revoke(conn, source, payment.subscription.as_deref(), now).await?;
                }
            }
        }
    }
    let outcome = if acted {
        Processed::Processed
    } else {
        Processed::Ignored
    };
    let status = if acted { "processed" } else { "ignored" };
    events::finish(&mut *conn, id, status, now).await?;
    Ok(outcome)
}

/// Takes the subscription's advisory lock for the rest of the transaction.
async fn lock(
    conn: &mut PgConnection,
    provider: &str,
    subscription: &str,
) -> Result<(), sqlx::Error> {
    let key = format!("payments:{provider}:{subscription}");
    sqlx::query!("select pg_advisory_xact_lock(hashtextextended($1, 0))", key)
        .execute(&mut *conn)
        .await
        .map(|_| ())
}

/// Reads the subscription's state and brings its licence in line;
/// `checkout` is ours when the event names it.
pub async fn sync_subscription(
    conn: &mut PgConnection,
    source: &dyn EventSource,
    subscription: &str,
    checkout: Option<Uuid>,
    now: OffsetDateTime,
) -> Result<LicenceChange, ProcessError> {
    let provider = source.id();
    lock(conn, provider, subscription).await?;
    let snapshot = source.subscription(subscription).await?;
    let price = plans::find_price(&mut *conn, provider, &snapshot.price_id)
        .await?
        .ok_or_else(|| {
            ProcessError::Permanent(format!(
                "subscription {subscription} bills price {}, which no plan has",
                snapshot.price_id
            ))
        })?;
    let checkout_id = checkout.or(snapshot.checkout);
    let checkout = match checkout_id {
        Some(id) => checkouts::find_for_update(&mut *conn, id).await?,
        None => None,
    };
    let licence =
        licences::find_by_subscription_for_update(&mut *conn, provider, subscription).await?;
    let before = licence.as_ref().map(state_of).transpose()?;
    let change = match (reconcile(before.as_ref(), &snapshot, now), licence) {
        (Decision::Wait, _) => LicenceChange::Waiting,
        (Decision::Issue(state), None) => {
            let checkout = checkout.ok_or_else(|| {
                ProcessError::Permanent(match checkout_id {
                    Some(id) => format!(
                        "subscription {subscription} is paid, but its checkout {id} is gone \
                         (the account was deleted?): no one to give the key to"
                    ),
                    None => format!(
                        "subscription {subscription} is paid, but names no checkout of ours"
                    ),
                })
            })?;
            let id = issue(
                conn,
                provider,
                &snapshot,
                &checkout,
                &price.plan_id,
                state,
                now,
            )
            .await?;
            LicenceChange::Issued(id)
        }
        (decision, Some(mut licence)) => {
            let state = match decision {
                Decision::Update(state) => Some(state),
                _ => None,
            };
            // A switch of price changes the plan, whatever else changed.
            let plan_moved = licence.plan_id != price.plan_id;
            if state.is_none() && !plan_moved {
                LicenceChange::Unchanged
            } else {
                if let Some(state) = state {
                    state.status.as_str().clone_into(&mut licence.status);
                    licence.paid_until = state.paid_until;
                    licence.cancel_at_period_end = state.cancel_at_period_end;
                    licence.past_due_since = state.past_due_since;
                    licence.ended_at = state.ended_at;
                }
                licence.plan_id.clone_from(&price.plan_id);
                licences::apply_subscription(&mut *conn, &licence).await?;
                LicenceChange::Updated(licence.id)
            }
        }
        // `Issue` only comes without a licence; `Keep` and `Update` with one.
        (_, None) => LicenceChange::Waiting,
    };
    // A checkout whose licence exists is completed, whichever event came first.
    if let Some(checkout) = checkout_id {
        let licence = match change {
            LicenceChange::Issued(id) | LicenceChange::Updated(id) => Some(id),
            _ => licences::find_by_subscription_for_update(&mut *conn, provider, subscription)
                .await?
                .map(|l| l.id),
        };
        if let Some(licence) = licence {
            complete_open(conn, checkout, licence, now).await?;
        }
    }
    Ok(change)
}

fn state_of(licence: &SubscriptionLicence) -> Result<LicenceState, ProcessError> {
    Ok(LicenceState {
        status: LicenceStatus::parse(&licence.status).ok_or_else(|| {
            ProcessError::Permanent(format!("licence {}: status {}", licence.id, licence.status))
        })?,
        paid_until: licence.paid_until,
        cancel_at_period_end: licence.cancel_at_period_end,
        past_due_since: licence.past_due_since,
        ended_at: licence.ended_at,
    })
}

async fn complete_open(
    conn: &mut PgConnection,
    checkout: Uuid,
    licence: Uuid,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    if let Some(row) = checkouts::find_for_update(&mut *conn, checkout).await?
        && row.status == "open"
    {
        checkouts::complete(&mut *conn, checkout, licence, now).await?;
    }
    Ok(())
}

/// The first licence of a paid subscription, for the person who started the
/// checkout.
async fn issue(
    conn: &mut PgConnection,
    provider: &str,
    snapshot: &SubscriptionSnapshot,
    checkout: &checkouts::CheckoutRow,
    plan_id: &str,
    state: LicenceState,
    now: OffsetDateTime,
) -> Result<Uuid, ProcessError> {
    if checkout.provider != provider {
        return Err(ProcessError::Permanent(format!(
            "checkout {} was started at {}, the subscription is at {provider}",
            checkout.id, checkout.provider
        )));
    }
    let plan = plans::find(&mut *conn, plan_id)
        .await?
        .ok_or_else(|| ProcessError::Permanent(format!("no plan `{plan_id}`")))?;
    let seats = plan
        .seats
        .ok_or_else(|| ProcessError::Permanent(format!("plan `{plan_id}` has no seats")))?;
    let customer = customer_of(conn, provider, &snapshot.customer, checkout.user_id).await?;
    let lid = key::new_lid().map_err(|err| ProcessError::Retry(err.to_string()))?;
    let new_key = key::generate().map_err(|err| ProcessError::Retry(err.to_string()))?;
    let id = licences::insert(
        &mut *conn,
        &NewLicence {
            lid: &lid,
            user_id: Some(checkout.user_id),
            customer_id: Some(customer),
            plan_id,
            kind: "subscription",
            seats,
            paid_until: state.paid_until,
            cancel_at_period_end: state.cancel_at_period_end,
            provider: Some(provider),
            provider_subscription_id: Some(&snapshot.id),
            key_hash: &new_key.hash,
            key_hint: &new_key.hint,
            note: None,
        },
    )
    .await?;
    reveals::insert(&mut *conn, id, &new_key.display, now, now + REVEAL_FOR).await?;
    tracing::info!(lid = %lid, plan = plan_id, provider, "licence issued from a checkout");
    Ok(id)
}

/// The provider's customer of `user_id`: found, or recorded now. A customer
/// tied to someone else, or a second customer of the same person, stops the
/// issue: one account is one customer.
async fn customer_of(
    conn: &mut PgConnection,
    provider: &str,
    provider_customer_id: &str,
    user_id: Uuid,
) -> Result<Uuid, ProcessError> {
    if let Some(customer) =
        customers::find_by_provider_id(&mut *conn, provider, provider_customer_id).await?
    {
        return if customer.user_id == Some(user_id) {
            Ok(customer.id)
        } else {
            Err(ProcessError::Permanent(format!(
                "customer {provider_customer_id} at {provider} belongs to another account"
            )))
        };
    }
    if customers::find_of_user(&mut *conn, user_id, provider)
        .await?
        .is_some()
    {
        return Err(ProcessError::Permanent(format!(
            "the account already has another customer at {provider} than {provider_customer_id}"
        )));
    }
    Ok(customers::insert(&mut *conn, user_id, provider, provider_customer_id).await?)
}

/// A full refund or a lost dispute revokes the subscription's licence.
async fn revoke(
    conn: &mut PgConnection,
    source: &dyn EventSource,
    subscription: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), ProcessError> {
    let Some(subscription) = subscription else {
        return Ok(());
    };
    lock(conn, source.id(), subscription).await?;
    if let Some(licence) =
        licences::find_by_subscription_for_update(&mut *conn, source.id(), subscription).await?
    {
        licences::revoke_for_refund(&mut *conn, licence.id, now).await?;
        tracing::info!(
            provider = source.id(),
            "licence revoked after a refund or a lost dispute"
        );
    }
    Ok(())
}
