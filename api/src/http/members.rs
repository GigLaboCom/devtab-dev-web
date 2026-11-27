//! A team licence's members, for its owner (the buyer, the licence's user):
//! who is invited, invite an address, remove one. A team of N members is the
//! owner and up to N − 1 invited addresses; an invited person signs in with
//! the address and connects their browsers from their account ("Connect with
//! your account"). Removing a member frees the seats of the browsers they
//! connected so. Anyone but the owner: 404, as for a licence that is not there.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::{AppError, ErrorBody};
use super::json::JsonBody;
use crate::auth::CurrentUser;
use crate::licence::wire;
use crate::state::AppState;
use crate::store::licences::LicenceRow;
use crate::store::{installations, licences, members, plans};

/// One invited address.
#[derive(Debug, Serialize, ToSchema)]
pub struct Member {
    pub email: String,
    /// The person has signed in with it and connected.
    pub joined: bool,
}

/// A team's members.
#[derive(Debug, Serialize, ToSchema)]
pub struct Team {
    /// Members bought, the owner included.
    pub members: i32,
    /// The invited addresses, at most `members` − 1.
    pub invited: Vec<Member>,
}

/// An address to invite.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Invite {
    pub email: String,
}

/// The owner's team licence, or 404 / 409 `not-a-team`.
async fn owned_team(
    state: &AppState,
    user: CurrentUser,
    lid: &str,
) -> Result<LicenceRow, AppError> {
    if !wire::is_lid(lid) {
        return Err(AppError::NotFound);
    }
    let pool = state.store.pool();
    let licence = licences::find_by_lid(pool, lid)
        .await?
        .filter(|l| l.user_id == Some(user.id))
        .ok_or(AppError::NotFound)?;
    let team = plans::find(pool, &licence.plan_id)
        .await?
        .is_some_and(|plan| plan.min_members.is_some());
    if team {
        Ok(licence)
    } else {
        Err(AppError::NotATeam)
    }
}

/// An address as stored: trimmed, lower case, one `@` with text on both sides.
fn address(text: &str) -> Option<String> {
    let email = text.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let ok = (3..=254).contains(&email.len())
        && !local.is_empty()
        && domain.contains('.')
        && !domain.contains('@')
        && !email.chars().any(char::is_whitespace);
    ok.then_some(email)
}

/// `GET /internal/licences/{lid}/members`.
#[utoipa::path(
    get,
    path = "/internal/licences/{lid}/members",
    tag = "licences",
    security(("bearer" = [])),
    params(("lid" = String, Path, description = "The team licence")),
    responses(
        (status = 200, description = "The team", body = Team),
        (status = 401, description = "No access token, or one that does not pass", body = ErrorBody),
        (status = 404, description = "No such licence of this person", body = ErrorBody),
        (status = 409, description = "`not-a-team`", body = ErrorBody),
    ),
)]
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(lid): Path<String>,
) -> Result<Json<Team>, AppError> {
    let licence = owned_team(&state, user, &lid).await?;
    let pool = state.store.pool();
    let count = licences::members_of(pool, licence.id).await?.unwrap_or(1);
    let invited = members::of_licence(pool, licence.id)
        .await?
        .into_iter()
        .map(|m| Member {
            email: m.email,
            joined: m.user_id.is_some(),
        })
        .collect();
    Ok(Json(Team {
        members: count,
        invited,
    }))
}

/// `POST /internal/licences/{lid}/members`.
#[utoipa::path(
    post,
    path = "/internal/licences/{lid}/members",
    tag = "licences",
    security(("bearer" = [])),
    params(("lid" = String, Path, description = "The team licence")),
    request_body = Invite,
    responses(
        (status = 201, description = "Invited (or already invited)"),
        (status = 400, description = "Not an e-mail address", body = ErrorBody),
        (status = 401, description = "No access token, or one that does not pass", body = ErrorBody),
        (status = 404, description = "No such licence of this person", body = ErrorBody),
        (status = 409, description = "`not-a-team`, or `team-full`: every member bought is invited", body = ErrorBody),
    ),
)]
pub async fn invite(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(lid): Path<String>,
    JsonBody(body): JsonBody<Invite>,
) -> Result<StatusCode, AppError> {
    let email = address(&body.email)
        .ok_or_else(|| AppError::BadRequest("not an e-mail address".to_owned()))?;
    let licence = owned_team(&state, user, &lid).await?;
    let mut tx = state.store.pool().begin().await?;
    // The licence row locked, so that two invitations at once cannot pass the limit.
    let locked = licences::find_by_lid_for_update(&mut *tx, &licence.lid)
        .await?
        .ok_or(AppError::NotFound)?;
    let count = licences::members_of(&mut *tx, locked.id)
        .await?
        .unwrap_or(1);
    let invited = members::of_licence(&mut *tx, locked.id).await?;
    if !invited.iter().any(|m| m.email == email) {
        if i32::try_from(invited.len()).unwrap_or(i32::MAX) >= count - 1 {
            return Err(AppError::TeamFull);
        }
        members::add(&mut *tx, locked.id, &email, state.clock.now()).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::CREATED)
}

/// `DELETE /internal/licences/{lid}/members/{email}`.
#[utoipa::path(
    delete,
    path = "/internal/licences/{lid}/members/{email}",
    tag = "licences",
    security(("bearer" = [])),
    params(
        ("lid" = String, Path, description = "The team licence"),
        ("email" = String, Path, description = "The member's address"),
    ),
    responses(
        (status = 204, description = "Removed; the browsers they connected are disconnected"),
        (status = 401, description = "No access token, or one that does not pass", body = ErrorBody),
        (status = 404, description = "No such licence of this person, or no such member", body = ErrorBody),
        (status = 409, description = "`not-a-team`", body = ErrorBody),
    ),
)]
pub async fn remove(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((lid, email)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let licence = owned_team(&state, user, &lid).await?;
    let email = address(&email).ok_or(AppError::NotFound)?;
    let mut tx = state.store.pool().begin().await?;
    let person = members::remove(&mut *tx, licence.id, &email)
        .await?
        .ok_or(AppError::NotFound)?;
    if let Some(person) = person {
        installations::release_of_user(&mut *tx, licence.id, person, state.clock.now()).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::address;

    #[test]
    fn addresses_are_trimmed_lowered_and_checked() {
        assert_eq!(
            address(" Ann@Example.Invalid ").as_deref(),
            Some("ann@example.invalid")
        );
        for bad in [
            "",
            "ann",
            "@example.invalid",
            "ann@",
            "ann@localhost",
            "a b@example.invalid",
            "a@b@c.d",
        ] {
            assert_eq!(address(bad), None, "{bad}");
        }
    }
}
