//! The purchase, for the site's pages: a signed-in person starts a checkout
//! of a plan (`POST /internal/checkouts`) and the return page asks how it
//! went (`GET /internal/checkouts/{id}`).
//!
//! Only verified provider events change anything; these routes record the
//! checkout, ask the provider to start its side, and read. The key a
//! completed checkout led to is answered once, and only to the person who
//! started it.

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
    let customer = customers::find_of_user(pool, user.id, stripe::ID).await?;
    let locale = body.locale.code();
    let id = checkouts::insert(
        pool,
        &checkouts::NewCheckout {
            user_id: Some(user.id),
            plan_id: &plan.id,
            price_id: price.id,
            provider: stripe::ID,
            locale,
            created_at: now,
            expires_at: now + CHECKOUT_OPEN_FOR,
            claim_hash: None,
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
    let (redirect_url, embedded) = match started.step {
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
    };
    let mut response = (
        StatusCode::CREATED,
        Json(StartedCheckout {
            checkout: id,
            redirect_url,
            embedded,
        }),
    )
        .into_response();
    no_store(&mut response);
    Ok(response)
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
    let answer = match (checkout.status.as_str(), checkout.licence_id) {
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
    };
    let mut response = Json(answer).into_response();
    no_store(&mut response);
    Ok(response)
}

fn no_store(response: &mut Response) {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
}
