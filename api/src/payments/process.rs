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
//!
//! A guest checkout (started without an account) has no person yet: when its
//! subscription is paid, the buyer is the user of the address they typed in
//! the provider's checkout (read from the provider, trimmed and lower case),
//! found in Supabase Auth or invited by mail ([`Accounts`]), never invited
//! twice. That happens inside the subscription's lock and the job's
//! transaction, and the lookup comes first, so a job tried again after a
//! failure finds the user the first attempt invited instead of inviting
//! again. A buyer who had an account gets no invite; a `mail_licence` job is
//! queued for them instead, which waits for the mail adapter.

use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::reconcile::{Decision, LicenceState, LicenceStatus, reconcile};
use super::{EventSource, ProviderError, Signal, SubscriptionSnapshot};
use crate::accounts::{AccountError, Accounts, Invitation, normal_email};
use crate::jobs::worker::MAIL_LICENCE;
use crate::licence::key;
use crate::store::licences::{NewLicence, SubscriptionLicence};
use crate::store::{checkouts, customers, events, jobs, licences, plans, reveals, users};

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

/// Processes the stored event `id` in the caller's transaction; `accounts`
/// invites the buyers of guest checkouts (none: such a checkout waits).
pub async fn process_event(
    conn: &mut PgConnection,
    source: &dyn EventSource,
    accounts: Option<&Accounts>,
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
                sync_subscription(conn, source, accounts, &subscription, Some(checkout), now)
                    .await?;
            }
            Signal::SubscriptionChanged { subscription }
            | Signal::InvoicePaid { subscription }
            | Signal::PaymentFailed { subscription } => {
                acted = true;
                sync_subscription(conn, source, accounts, &subscription, None, now).await?;
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
    lock_key(conn, &format!("payments:{provider}:{subscription}")).await
}

/// Takes the advisory lock named `key` for the rest of the transaction.
async fn lock_key(conn: &mut PgConnection, key: &str) -> Result<(), sqlx::Error> {
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
    accounts: Option<&Accounts>,
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
            let id = issue_from_checkout(
                conn,
                source,
                accounts,
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
            // A switch of price changes the plan, and a team's size its seats,
            // whatever else changed.
            let (members, seats) = team_size(conn, &price.plan_id, snapshot.quantity).await?;
            let resized = licence.members != members || licence.seats != seats;
            let plan_moved = licence.plan_id != price.plan_id;
            if state.is_none() && !plan_moved && !resized {
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
                licence.members = members;
                licence.seats = seats;
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

/// The first licence of a paid subscription from our `checkout`: for the
/// person who started it, or a guest checkout's buyer, found or invited now.
/// A buyer who had an account gets no invite, and the mail that says the
/// licence is in their account is queued (`mail_licence`).
#[allow(clippy::too_many_arguments)]
async fn issue_from_checkout(
    conn: &mut PgConnection,
    source: &dyn EventSource,
    accounts: Option<&Accounts>,
    snapshot: &SubscriptionSnapshot,
    checkout: &checkouts::CheckoutRow,
    plan_id: &str,
    state: LicenceState,
    now: OffsetDateTime,
) -> Result<Uuid, ProcessError> {
    let provider = source.id();
    let Some(user) = checkout.user_id else {
        let buyer = ensure_buyer(conn, source, accounts, checkout).await?;
        checkouts::set_buyer(&mut *conn, checkout.id, buyer.user(), buyer.as_str()).await?;
        let id = issue(
            conn,
            provider,
            snapshot,
            checkout,
            buyer.user(),
            plan_id,
            state,
            now,
        )
        .await?;
        if let Buyer::Existing(_) = buyer {
            // No invite for an address that has an account: the licence is in
            // it. The mail that says so waits for the mail adapter.
            jobs::enqueue(
                &mut *conn,
                MAIL_LICENCE,
                &serde_json::json!({ "licence": id, "checkout": checkout.id }),
                now,
            )
            .await?;
        }
        return Ok(id);
    };
    issue(
        conn, provider, snapshot, checkout, user, plan_id, state, now,
    )
    .await
}

/// The first licence of a paid subscription, for `owner`: the person who
/// started the checkout, or a guest checkout's buyer.
#[allow(clippy::too_many_arguments)]
async fn issue(
    conn: &mut PgConnection,
    provider: &str,
    snapshot: &SubscriptionSnapshot,
    checkout: &checkouts::CheckoutRow,
    owner: Uuid,
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
    let (members, seats) = team_size(conn, plan_id, snapshot.quantity).await?;
    let customer = customer_of(
        conn,
        provider,
        &snapshot.customer,
        owner,
        checkout.is_guest(),
    )
    .await?;
    let lid = key::new_lid().map_err(|err| ProcessError::Retry(err.to_string()))?;
    let new_key = key::generate().map_err(|err| ProcessError::Retry(err.to_string()))?;
    let id = licences::insert(
        &mut *conn,
        &NewLicence {
            lid: &lid,
            user_id: Some(owner),
            customer_id: Some(customer),
            plan_id,
            kind: "subscription",
            seats,
            members,
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

/// A licence's members and seats for `quantity` of `plan_id`: a team plan
/// gives its seats to each member; a personal plan is one person, whatever the
/// quantity.
async fn team_size(
    conn: &mut PgConnection,
    plan_id: &str,
    quantity: u32,
) -> Result<(i32, i32), ProcessError> {
    let plan = plans::find(&mut *conn, plan_id)
        .await?
        .ok_or_else(|| ProcessError::Permanent(format!("no plan `{plan_id}`")))?;
    let per_member = plan
        .seats
        .ok_or_else(|| ProcessError::Permanent(format!("plan `{plan_id}` has no seats")))?;
    if plan.min_members.is_none() {
        return Ok((1, per_member));
    }
    let members = i32::try_from(quantity.clamp(1, 1000)).unwrap_or(1);
    Ok((members, per_member.saturating_mul(members)))
}

/// The provider's customer of `user_id`: found, or recorded now. A customer
/// tied to someone else stops the issue, and so does a second customer of a
/// person who bought signed in (the checkout named their customer). A guest
/// checkout could name none, so its buyer may have had one already.
async fn customer_of(
    conn: &mut PgConnection,
    provider: &str,
    provider_customer_id: &str,
    user_id: Uuid,
    guest: bool,
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
    if !guest
        && customers::find_of_user(&mut *conn, user_id, provider)
            .await?
            .is_some()
    {
        return Err(ProcessError::Permanent(format!(
            "the account already has another customer at {provider} than {provider_customer_id}"
        )));
    }
    Ok(customers::insert(&mut *conn, user_id, provider, provider_customer_id).await?)
}

/// How a guest checkout's buyer got the account the licence goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buyer {
    /// A new user, invited by mail now.
    Invited(Uuid),
    /// The address had an account already: nobody is invited.
    Existing(Uuid),
}

impl Buyer {
    pub fn user(self) -> Uuid {
        match self {
            Self::Invited(id) | Self::Existing(id) => id,
        }
    }

    /// As `app.checkouts.buyer` records it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invited(_) => "invited",
            Self::Existing(_) => "existing",
        }
    }
}

/// The user a paid guest checkout belongs to: the account of the address the
/// buyer typed at the provider, or a new one invited by mail.
///
/// The address is only trimmed and lower-cased (no dots or `+tags` dropped):
/// the account must be the one the person signs in to with that address.
/// Under an advisory lock on the address, so that two purchases with one
/// address never both invite it; the lookup comes before the invite, so a
/// retry after a failed transaction finds the user instead of inviting again
/// (Supabase Auth would send an unconfirmed address another invite).
async fn ensure_buyer(
    conn: &mut PgConnection,
    source: &dyn EventSource,
    accounts: Option<&Accounts>,
    checkout: &checkouts::CheckoutRow,
) -> Result<Buyer, ProcessError> {
    let accounts = accounts.ok_or_else(|| {
        ProcessError::Retry(format!(
            "checkout {} was started without an account, and no Supabase secret key is set to \
             invite its buyer (DEVTAB_SUPABASE_SECRET_KEY_FILE)",
            checkout.id
        ))
    })?;
    let session = checkout.provider_checkout_id.as_deref().ok_or_else(|| {
        ProcessError::Permanent(format!(
            "guest checkout {} has no checkout at the provider to read the buyer's address from",
            checkout.id
        ))
    })?;
    let typed = source.checkout_email(session).await?.ok_or_else(|| {
        ProcessError::Permanent(format!(
            "guest checkout {}: the provider has no address of the buyer",
            checkout.id
        ))
    })?;
    let email = normal_email(&typed).ok_or_else(|| {
        ProcessError::Permanent(format!(
            "guest checkout {}: the buyer's address at the provider is not an address",
            checkout.id
        ))
    })?;
    lock_key(conn, &format!("accounts:{email}")).await?;
    if let Some(user) = users::id_by_email(&mut *conn, &email).await? {
        tracing::info!(checkout = %checkout.id, "a guest checkout's buyer has an account: no invite");
        return Ok(Buyer::Existing(user));
    }
    let team = plans::find(&mut *conn, &checkout.plan_id)
        .await?
        .is_some_and(|plan| plan.min_members.is_some());
    let invitation = Invitation {
        locale: &checkout.locale,
        checkout: checkout.id,
        team,
    };
    match accounts.invite(&email, &invitation).await {
        Ok(user) => {
            tracing::info!(checkout = %checkout.id, "a guest checkout's buyer was invited");
            Ok(Buyer::Invited(user))
        }
        // Made in the meantime, elsewhere (a sign-up): theirs, uninvited.
        Err(AccountError::Exists) => users::id_by_email(&mut *conn, &email)
            .await?
            .map(Buyer::Existing)
            .ok_or_else(|| {
                ProcessError::Retry(format!(
                    "guest checkout {}: Supabase Auth says the buyer's address has an account, \
                     which is not found yet",
                    checkout.id
                ))
            }),
        Err(err @ AccountError::Unavailable(_)) => Err(ProcessError::Retry(err.to_string())),
        Err(err @ AccountError::Refused(_)) => Err(ProcessError::Permanent(err.to_string())),
    }
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
