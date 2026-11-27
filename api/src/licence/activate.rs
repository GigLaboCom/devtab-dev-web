//! Activation: an installation id becomes a token and the installation's
//! secret, for the licence a key names (`POST /v1/activate`) or for the
//! signed-in person's licence (the site connects the extension, `POST
//! /internal/licences/connect`). Both take the same seat, issue the same
//! secret and sign the same token.

use super::token::{self, Status};
use super::{entitlement, key, seats, secret};
use crate::http::error::AppError;
use crate::state::AppState;
use crate::store::licences::LicenceRow;
use crate::store::{installations, licences};
use uuid::Uuid;

/// A successful activation.
pub struct Activated {
    pub token: String,
    pub secret: String,
    pub lid: String,
}

// The secret must never reach a log through `{:?}`.
impl std::fmt::Debug for Activated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Activated")
            .field("lid", &self.lid)
            .finish_non_exhaustive()
    }
}

/// Activates `inst` with the key whose hash is `key_hash`, in one
/// transaction with the licence row locked:
///
/// 1. no licence with this key: `key-unknown`;
/// 2. the key is blocked: `key-blocked`;
/// 3. the token signed now would not be `active` (ended, refunded, or the
///    payment's grace is over): `licence-inactive`;
/// 4. the same installation, still active: a new secret and token, no new
///    seat (the old secret stops working);
/// 5. otherwise a seat is taken if one is free and the limit on new
///    activations allows it, else `seat-limit`.
pub async fn activate(
    state: &AppState,
    key_hash: &[u8; 32],
    inst: &str,
) -> Result<Activated, AppError> {
    let mut tx = state.store.pool().begin().await?;
    let Some(licence) = licences::find_by_key_hash_for_update(&mut *tx, key_hash).await? else {
        return Err(AppError::KeyUnknown);
    };
    let done = admit(state, &mut tx, &licence, inst).await?;
    tx.commit().await?;
    Ok(done)
}

/// Activates `inst` with the signed-in person's licence: `lid` when given
/// (it must be theirs), else their one usable licence. None usable:
/// `no-licence`; more than one and no `lid`: `choose-licence`. Then as
/// [`activate`] from step 2.
pub async fn activate_for_user(
    state: &AppState,
    user: Uuid,
    inst: &str,
    lid: Option<&str>,
) -> Result<Activated, AppError> {
    let iat = state.clock.now().unix_timestamp();
    let spans = &state.licence.spans;
    let usable = |l: &LicenceRow| {
        l.key_blocked_at.is_none()
            && l.revoke_reason.as_deref() != Some("key-blocked")
            && entitlement(l).is_active_at(iat, spans)
    };
    let mut tx = state.store.pool().begin().await?;
    let mine = licences::of_user(&mut *tx, user).await?;
    let chosen = if let Some(lid) = lid {
        mine.iter()
            .find(|l| l.lid == lid)
            .ok_or(AppError::NotFound)?
            .lid
            .clone()
    } else {
        let usable: Vec<&LicenceRow> = mine.iter().filter(|l| usable(l)).collect();
        match usable.as_slice() {
            [] => return Err(AppError::NoLicence),
            [one] => one.lid.clone(),
            _ => return Err(AppError::ChooseLicence),
        }
    };
    let licence = licences::find_by_lid_for_update(&mut *tx, &chosen)
        .await?
        .filter(|l| l.user_id == Some(user))
        .ok_or(AppError::NotFound)?;
    let done = admit(state, &mut tx, &licence, inst).await?;
    tx.commit().await?;
    Ok(done)
}

/// Steps 2–5 of [`activate`] for a licence locked in `tx`.
async fn admit(
    state: &AppState,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    licence: &LicenceRow,
    inst: &str,
) -> Result<Activated, AppError> {
    let now = state.clock.now();
    let iat = now.unix_timestamp();
    let settings = &state.licence;
    if licence.key_blocked_at.is_some() || licence.revoke_reason.as_deref() == Some("key-blocked") {
        return Err(AppError::KeyBlocked);
    }
    let rights = entitlement(licence);
    if !rights.is_active_at(iat, &settings.spans) {
        return Err(AppError::LicenceInactive);
    }
    let new_secret = secret::generate()?;
    let existing = installations::find(&mut **tx, licence.id, inst).await?;
    match existing {
        Some(installation) if installation.status == "active" => {
            installations::replace_secret(&mut **tx, installation.id, &new_secret.hash, now)
                .await?;
        }
        _ => {
            seats::admit(tx, licence.id, licence.seats, now, settings).await?;
            installations::activate(&mut **tx, licence.id, inst, &new_secret.hash, now).await?;
        }
    }
    let claims = token::issue(&licence.lid, inst, iat, &rights, &settings.spans);
    debug_assert_eq!(claims.status, Status::Active);
    Ok(Activated {
        token: state.signer.sign(&claims),
        secret: new_secret.secret,
        lid: licence.lid.clone(),
    })
}

/// The hash a raw key is looked up by, or `key-unknown` when it is not a key
/// of the server's format.
pub fn key_hash(raw: &str) -> Result<[u8; 32], AppError> {
    key::canonical(raw)
        .map(|canonical| key::hash(&canonical))
        .ok_or(AppError::KeyUnknown)
}
