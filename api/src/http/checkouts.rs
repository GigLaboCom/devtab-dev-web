//! The purchase, for the site's pages: a signed-in person starts a checkout
//! of a plan (`POST /internal/checkouts`) and the return page asks how it
//! went (`GET /internal/checkouts/{id}`). Where the deployment allows it
//! (`DEVTAB_GUEST_CHECKOUT`), anyone starts one without an account
//! (`POST /internal/checkouts/guest`): the answer carries a secret that the
//! site keeps in a cookie of that browser, and only that secret reads the
//! checkout (`POST /internal/checkouts/{id}/claim`). The buyer's account comes
//! from the payment (`payments::process`).
//!
//! Only verified provider events change anything; these routes record the
//! checkout, ask the provider to start its side, and read. The key a
//! completed checkout led to is answered once, and only to the person who
//! started it or the browser holding the guest checkout's secret.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use serde::{Deserialize, Serialize};
use time::Duration;
use utoipa::ToSchema;
use uuid::Uuid;

use super::error::{AppError, ErrorBody};
use super::json::JsonBody;
use super::plans::Locale;
use crate::auth::CurrentUser;
use crate::licence::{secret, wire};
use crate::payments::stripe::{self, CheckoutRequest, CheckoutStep};
use crate::state::AppState;
use crate::store::{checkouts, customers, licences, plans, reveals};

/// How long a checkout stays open: the provider's own default.
pub const CHECKOUT_OPEN_FOR: Duration = Duration::hours(24);

/// A checkout to start.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewCheckout {
    /// The plan's id, such as `pro-monthly`.
    pub plan: String,
    /// The language of the provider's page, the return page and the mail.
    #[serde(default)]
    pub locale: Locale,
    /// A team plan: how many members to start with (its smallest team when
    /// absent; the buyer may change it on the provider's page). Not for a
    /// personal plan.
    pub members: Option<u32>,
}

/// Stripe's form for the site's own page.
#[derive(Debug, Serialize, ToSchema)]
pub struct EmbeddedCheckout {
    pub client_secret: String,
    /// Public by design.
    pub publishable_key: String,
    /// Stripe.js of this API version mounts the form.
    pub api_version: String,
}

/// A started checkout: where to send the buyer, or the form to mount.
#[derive(Debug, Serialize, ToSchema)]
pub struct StartedCheckout {
    #[schema(value_type = String, format = "uuid")]
    pub checkout: Uuid,
    /// The provider's page (a hosted checkout); answer it with 303.
    pub redirect_url: Option<String>,
    /// The form (an embedded checkout).
    pub embedded: Option<EmbeddedCheckout>,
}

/// `POST /internal/checkouts`: starts a checkout of a plan for the
/// signed-in person.
#[utoipa::path(
    post,
    path = "/internal/checkouts",
    tag = "purchase",
    security(("bearer" = [])),
    request_body = NewCheckout,
    responses(
        (status = 201, description = "Started", body = StartedCheckout),
        (status = 401, description = "No access token, or one that does not pass", body = ErrorBody),
        (status = 404, description = "`plan-unknown`", body = ErrorBody),
        (status = 409, description = "`plan-unavailable`: not sold now", body = ErrorBody),
        (status = 503, description = "`payments-unavailable`: no provider, or it did not start", body = ErrorBody),
    ),
)]
pub async fn start(
    State(state): State<AppState>,
    user: CurrentUser,
    JsonBody(body): JsonBody<NewCheckout>,
) -> Result<Response, AppError> {
    let (checkout, step) = begin(&state, Starter::Person(user.id), &body).await?;
    let (redirect_url, embedded) = continuation(step);
    Ok(created(StartedCheckout {
        checkout,
        redirect_url,
        embedded,
    }))
}

/// A started guest checkout: as [`StartedCheckout`], with the secret that
/// reads it.
#[derive(Debug, Serialize, ToSchema)]
pub struct StartedGuestCheckout {
    #[schema(value_type = String, format = "uuid")]
    pub checkout: Uuid,
    /// The provider's page (a hosted checkout); answer it with 303.
    pub redirect_url: Option<String>,
    /// The form (an embedded checkout).
    pub embedded: Option<EmbeddedCheckout>,
    /// The secret that reads this checkout (`POST /internal/checkouts/{id}/claim`):
    /// keep it in an `HttpOnly` cookie of the browser that started it, never
    /// in an address or a page. Shown once; only its hash is stored.
    pub claim: String,
}

/// `POST /internal/checkouts/guest`: starts a checkout of a plan without an
/// account, where the deployment allows it (`DEVTAB_GUEST_CHECKOUT`). The
/// provider asks the buyer for an address; when the payment is through, the
/// licence goes to that address's account, made and invited by mail if
/// there is none. The site limits how often one client may start one.
#[utoipa::path(
    post,
    path = "/internal/checkouts/guest",
    tag = "purchase",
    request_body = NewCheckout,
    responses(
        (status = 201, description = "Started", body = StartedGuestCheckout),
        (status = 403, description = "`guest-checkout-off`: not offered here", body = ErrorBody),
        (status = 404, description = "`plan-unknown`", body = ErrorBody),
        (status = 409, description = "`plan-unavailable`: not sold now", body = ErrorBody),
        (status = 503, description = "`guest-checkout-unavailable`: no Supabase secret key to invite \
          the buyer with; `payments-unavailable`: no provider, or it did not start", body = ErrorBody),
    ),
)]
pub async fn start_guest(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<NewCheckout>,
) -> Result<Response, AppError> {
    if !state.guest_checkout {
        return Err(AppError::GuestCheckoutOff);
    }
    if state.accounts.is_none() {
        return Err(AppError::GuestCheckoutUnavailable);
    }
    let claim = secret::generate()?;
    let (checkout, step) = begin(&state, Starter::Guest(&claim.hash), &body).await?;
    let (redirect_url, embedded) = continuation(step);
    Ok(created(StartedGuestCheckout {
        checkout,
        redirect_url,
        embedded,
        claim: claim.secret,
    }))
}

/// Who starts a checkout.
#[derive(Debug, Clone, Copy)]
enum Starter<'a> {
    /// A signed-in person.
    Person(Uuid),
    /// Nobody signed in: the SHA-256 of the starting browser's secret.
    Guest(&'a [u8; 32]),
}

/// Records a checkout of `body`'s plan and starts the provider's side.
async fn begin(
    state: &AppState,
    who: Starter<'_>,
    body: &NewCheckout,
) -> Result<(Uuid, CheckoutStep), AppError> {
    let payments = state
        .payments
        .as_ref()
        .ok_or(AppError::PaymentsUnavailable)?;
    let now = state.clock.now();
    let pool = state.store.pool();
    let plan = plans::find(pool, &body.plan)
        .await?
        .ok_or(AppError::PlanUnknown)?;
    let on_sale =
        plan.kind == "subscription" && plan.available && plan.until.is_none_or(|until| until > now);
    if !on_sale {
        return Err(AppError::PlanUnavailable);
    }
    let (quantity, adjustable_from) = team_quantity(plan.min_members, body.members)?;
    let price = plans::active_price_of_plan(pool, &plan.id, stripe::ID)
        .await?
        .ok_or(AppError::PlanUnavailable)?;
    // A signed-in person's customer, so that the provider keeps one per
    // account. A guest checkout names none: the provider asks for the
    // address and makes a customer (a subscription checkout always does).
    let (user_id, claim_hash, customer) = match who {
        Starter::Person(user) => (
            Some(user),
            None,
            customers::find_of_user(pool, user, stripe::ID).await?,
        ),
        Starter::Guest(hash) => (None, Some(hash.as_slice()), None),
    };
    let locale = body.locale.code();
    let id = checkouts::insert(
        pool,
        &checkouts::NewCheckout {
            user_id,
            plan_id: &plan.id,
            price_id: price.id,
            provider: stripe::ID,
            locale,
            created_at: now,
            expires_at: now + CHECKOUT_OPEN_FOR,
            claim_hash,
        },
    )
    .await?;
    let prefix = if locale == "en" {
        String::new()
    } else {
        format!("/{locale}")
    };
    let site = &state.catalogue.site_url;
    let request = CheckoutRequest {
        checkout_id: id,
        price_id: price.provider_price_id,
        quantity,
        adjustable_from,
        customer: customer.map(|c| c.provider_customer_id),
        locale: Some(locale.to_owned()),
        success_url: format!("{site}{prefix}/pro/thanks/?ref={id}"),
        cancel_url: format!("{site}{prefix}/pro/"),
    };
    let started = payments
        .stripe
        .create_checkout(&request)
        .await
        .map_err(|err| {
            tracing::warn!(error = %err, "the provider did not start a checkout");
            AppError::PaymentsUnavailable
        })?;
    checkouts::set_provider_checkout(pool, id, &started.provider_checkout_id).await?;
    Ok((id, started.step))
}

/// Where the buyer goes on: the provider's page, or its form to mount.
fn continuation(step: CheckoutStep) -> (Option<String>, Option<EmbeddedCheckout>) {
    match step {
        CheckoutStep::Redirect { url } => (Some(url), None),
        CheckoutStep::Embedded {
            client_secret,
            publishable_key,
            api_version,
        } => (
            None,
            Some(EmbeddedCheckout {
                client_secret,
                publishable_key,
                api_version,
            }),
        ),
    }
}

fn created(body: impl Serialize) -> Response {
    let mut response = (StatusCode::CREATED, Json(body)).into_response();
    no_store(&mut response);
    response
}

/// How many to buy, and from what number the buyer may change it: a team
/// plan's members (its smallest team when not given), one for a personal plan.
fn team_quantity(
    min_members: Option<i16>,
    members: Option<u32>,
) -> Result<(u32, Option<u32>), AppError> {
    match (min_members, members) {
        (None, None) => Ok((1, None)),
        (None, Some(_)) => Err(AppError::BadRequest(
            "members is for a team plan only".to_owned(),
        )),
        (Some(min), members) => {
            let min = u32::try_from(min).unwrap_or(1);
            let members = members.unwrap_or(min);
            if (min..=1000).contains(&members) {
                Ok((members, Some(min)))
            } else {
                Err(AppError::BadRequest(format!(
                    "members must be {min} to 1000"
                )))
            }
        }
    }
}

/// Where a checkout stands.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckoutStatus {
    /// Waiting for the payment, or for the provider's word on it.
    Open,
    Completed,
    /// Not paid in time.
    Expired,
}

/// A checkout as its buyer sees it.
#[derive(Debug, Serialize, ToSchema)]
pub struct CheckoutState {
    pub status: CheckoutStatus,
    /// The licence key: only the first time a completed checkout is asked
    /// for, within a day of the licence; null ever after.
    #[schema(required = true)]
    pub key: Option<String>,
    /// The key's last four characters, once completed.
    #[schema(required = true)]
    pub key_hint: Option<String>,
}

/// `GET /internal/checkouts/{id}`: how the signed-in person's checkout went.
#[utoipa::path(
    get,
    path = "/internal/checkouts/{id}",
    tag = "purchase",
    security(("bearer" = [])),
    params(("id" = String, Path, format = "uuid", description = "The checkout, the `ref` of the return address")),
    responses(
        (status = 200, description = "The checkout's state", body = CheckoutState),
        (status = 401, description = "No access token, or one that does not pass", body = ErrorBody),
        (status = 404, description = "No such checkout of this person", body = ErrorBody),
    ),
)]
pub async fn read(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let now = state.clock.now();
    let pool = state.store.pool();
    let checkout = checkouts::find_of_user(pool, id, user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    let answer = state_of(pool, &checkout, now).await?;
    let mut response = Json(answer).into_response();
    no_store(&mut response);
    Ok(response)
}

/// What a checkout's buyer may see now: the key once, then its hint.
async fn state_of(
    pool: &sqlx::PgPool,
    checkout: &checkouts::CheckoutRow,
    now: time::OffsetDateTime,
) -> Result<CheckoutState, AppError> {
    Ok(match (checkout.status.as_str(), checkout.licence_id) {
        ("completed", Some(licence)) => CheckoutState {
            status: CheckoutStatus::Completed,
            key: reveals::reveal_once(pool, licence, now).await?,
            key_hint: licences::hint_of(pool, licence).await?,
        },
        ("open", _) if checkout.expires_at > now => CheckoutState {
            status: CheckoutStatus::Open,
            key: None,
            key_hint: None,
        },
        _ => CheckoutState {
            status: CheckoutStatus::Expired,
            key: None,
            key_hint: None,
        },
    })
}

/// The secret of a guest checkout, as the site kept it.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckoutClaim {
    /// `claim` of the started guest checkout.
    pub claim: String,
}

/// How a guest checkout's buyer got the account its licence is in.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum BuyerAccount {
    /// A new account, invited by mail: the buyer opens it from that mail.
    Invited,
    /// The address had an account: the licence is in it, nobody was invited.
    Existing,
}

/// A guest checkout as the browser that started it sees it.
#[derive(Debug, Serialize, ToSchema)]
pub struct GuestCheckoutState {
    pub status: CheckoutStatus,
    /// As [`CheckoutState::key`]: the first time it is asked for, by this
    /// browser or by the buyer's account, whichever comes first.
    #[schema(required = true)]
    pub key: Option<String>,
    #[schema(required = true)]
    pub key_hint: Option<String>,
    /// Once paid: whether the buyer was invited or had an account.
    #[schema(required = true)]
    pub account: Option<BuyerAccount>,
}

/// `POST /internal/checkouts/{id}/claim`: how a guest checkout went, for the
/// browser that started it (the secret its start answered).
#[utoipa::path(
    post,
    path = "/internal/checkouts/{id}/claim",
    tag = "purchase",
    params(("id" = String, Path, format = "uuid", description = "The checkout, the `ref` of the return address")),
    request_body = CheckoutClaim,
    responses(
        (status = 200, description = "The checkout's state", body = GuestCheckoutState),
        (status = 404, description = "No such guest checkout with this secret", body = ErrorBody),
    ),
)]
pub async fn claim(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    JsonBody(body): JsonBody<CheckoutClaim>,
) -> Result<Response, AppError> {
    if !wire::is_secret(&body.claim) {
        return Err(AppError::NotFound);
    }
    let now = state.clock.now();
    let pool = state.store.pool();
    let checkout = checkouts::find_by_claim(pool, id, &secret::hash(&body.claim))
        .await?
        .ok_or(AppError::NotFound)?;
    let seen = state_of(pool, &checkout, now).await?;
    let account = match checkout.buyer.as_deref() {
        Some("invited") => Some(BuyerAccount::Invited),
        Some("existing") => Some(BuyerAccount::Existing),
        _ => None,
    };
    let mut response = Json(GuestCheckoutState {
        status: seen.status,
        key: seen.key,
        key_hint: seen.key_hint,
        account,
    })
    .into_response();
    no_store(&mut response);
    Ok(response)
}

fn no_store(response: &mut Response) {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
}
