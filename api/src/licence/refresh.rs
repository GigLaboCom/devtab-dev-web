//! `POST /v1/refresh`: an installation proves itself with its secret and gets
//! a new token, which always says where it stands.

use super::token::{self, Entitlement, RevokeReason, Standing};
use super::{entitlement, seats, secret};
use crate::http::error::AppError;
use crate::state::AppState;
use crate::store::{installations, licences};

/// The token for this installation now. The first matching row wins:
///
/// | condition                                         | token                       |
/// |---------------------------------------------------|-----------------------------|
/// | no such licence or installation, wrong secret     | `revoked: unknown-installation` |
/// | the key is blocked                                | `revoked: key-blocked`      |
/// | refunded                                          | `revoked: refund`           |
/// | deactivated, or released from the site or by an administrator | `revoked: deactivated` |
/// | released for want of contact                      | a free seat: back in `active`; none: `revoked: seat-limit` |
/// | the licence ended                                 | `ended`                     |
/// | the licence is active                             | by the issuing rules (maybe `ended` when the payment's grace is over) |
///
/// The only side effects: when the installation was last seen, and an
/// installation released for want of contact coming back.
pub async fn refresh(
    state: &AppState,
    lid: &str,
    inst: &str,
    secret_text: &str,
) -> Result<String, AppError> {
    let now = state.clock.now();
    let iat = now.unix_timestamp();
    let spans = &state.licence.spans;
    let mut tx = state.store.pool().begin().await?;

    let unknown = || token::unknown_installation(lid, inst, iat, spans);
    let Some(licence) = licences::find_by_lid_for_update(&mut *tx, lid).await? else {
        return Ok(state.signer.sign(&unknown()));
    };
    let Some(installation) = installations::find(&mut *tx, licence.id, inst).await? else {
        return Ok(state.signer.sign(&unknown()));
    };
    if !secret::matches(secret_text, &installation.secret_hash) {
        return Ok(state.signer.sign(&unknown()));
    }

    let rights = entitlement(&licence);
    let revoked = |reason| Entitlement {
        standing: Standing::Revoked(reason),
        ..rights
    };
    let blocked =
        licence.key_blocked_at.is_some() || licence.revoke_reason.as_deref() == Some("key-blocked");
    let decided = if blocked {
        revoked(RevokeReason::KeyBlocked)
    } else if licence.revoke_reason.as_deref() == Some("refund") {
        revoked(RevokeReason::Refund)
    } else {
        match (
            installation.status.as_str(),
            installation.released_reason.as_deref(),
        ) {
            ("deactivated", _) | ("released", Some("manual" | "admin")) => {
                revoked(RevokeReason::Deactivated)
            }
            ("released", _)
                if !seats::has_free_seat(&mut tx, licence.id, licence.seats).await? =>
            {
                revoked(RevokeReason::SeatLimit)
            }
            _ => {
                // Active, or released for want of contact with a seat free.
                installations::touch(&mut *tx, installation.id, now).await?;
                rights
            }
        }
    };
    tx.commit().await?;
    Ok(state
        .signer
        .sign(&token::issue(lid, inst, iat, &decided, spans)))
}
