//! `POST /v1/activate`: a licence key and an installation id become a token
//! and the installation's secret.

use super::token::{self, Status};
use super::{entitlement, key, seats, secret};
use crate::http::error::AppError;
use crate::state::AppState;
use crate::store::{installations, licences};

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
    let now = state.clock.now();
    let iat = now.unix_timestamp();
    let settings = &state.licence;
    let mut tx = state.store.pool().begin().await?;

    let Some(licence) = licences::find_by_key_hash_for_update(&mut *tx, key_hash).await? else {
        return Err(AppError::KeyUnknown);
    };
    if licence.key_blocked_at.is_some() || licence.revoke_reason.as_deref() == Some("key-blocked") {
        return Err(AppError::KeyBlocked);
    }
    let rights = entitlement(&licence);
    if !rights.is_active_at(iat, &settings.spans) {
        return Err(AppError::LicenceInactive);
    }

    let new_secret = secret::generate()?;
    let existing = installations::find(&mut *tx, licence.id, inst).await?;
    match existing {
        Some(installation) if installation.status == "active" => {
            installations::replace_secret(&mut *tx, installation.id, &new_secret.hash, now).await?;
        }
        _ => {
            seats::admit(&mut tx, licence.id, licence.seats, now, settings).await?;
            installations::activate(&mut *tx, licence.id, inst, &new_secret.hash, now).await?;
        }
    }
    tx.commit().await?;

    let claims = token::issue(&licence.lid, inst, iat, &rights, &settings.spans);
    debug_assert_eq!(claims.status, Status::Active);
    Ok(Activated {
        token: state.signer.sign(&claims),
        secret: new_secret.secret,
        lid: licence.lid,
    })
}

/// The hash a raw key is looked up by, or `key-unknown` when it is not a key
/// of the server's format.
pub fn key_hash(raw: &str) -> Result<[u8; 32], AppError> {
    key::canonical(raw)
        .map(|canonical| key::hash(&canonical))
        .ok_or(AppError::KeyUnknown)
}
