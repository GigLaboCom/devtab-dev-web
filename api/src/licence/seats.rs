//! Seats: how many installations a licence may have active, the limit on new
//! activations, deactivation, and releasing installations that have not been
//! seen for a long time.

use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};

use super::{LicenceSettings, secret};
use crate::http::error::AppError;
use crate::store::installations::{self, Released};
use crate::store::licences;

/// The period the limit on new activations counts over.
pub const CHURN_PERIOD: Duration = Duration::days(30);

/// Whether one more installation may take a seat of the licence: fewer
/// active installations than seats, and fewer new activations in 30 days
/// than `activation_churn` × seats. Either refusal is `seat-limit`: a person
/// gets the same advice ("deactivate it in another browser").
pub async fn admit(
    conn: &mut PgConnection,
    licence_id: uuid::Uuid,
    seats: i32,
    now: OffsetDateTime,
    settings: &LicenceSettings,
) -> Result<(), AppError> {
    let seats = i64::from(seats);
    if installations::count_active(&mut *conn, licence_id).await? >= seats {
        return Err(AppError::SeatLimit);
    }
    let recent =
        installations::count_activated_since(&mut *conn, licence_id, now - CHURN_PERIOD).await?;
    if recent >= i64::from(settings.activation_churn) * seats {
        return Err(AppError::SeatLimit);
    }
    Ok(())
}

/// Whether a seat is free (an installation released for want of contact
/// comes back only then).
pub async fn has_free_seat(
    conn: &mut PgConnection,
    licence_id: uuid::Uuid,
    seats: i32,
) -> Result<bool, sqlx::Error> {
    Ok(installations::count_active(&mut *conn, licence_id).await? < i64::from(seats))
}

/// `POST /v1/deactivate`: when the licence, the installation and its secret
/// match, the installation is deactivated and its seat freed. Anything else
/// changes nothing; the answer is the same either way. Returns whether an
/// installation was deactivated.
pub async fn deactivate(
    conn: &mut PgConnection,
    lid: &str,
    inst: &str,
    secret_text: &str,
    now: OffsetDateTime,
) -> Result<bool, sqlx::Error> {
    let Some(licence) = licences::find_by_lid(&mut *conn, lid).await? else {
        return Ok(false);
    };
    let Some(installation) = installations::find(&mut *conn, licence.id, inst).await? else {
        return Ok(false);
    };
    if !secret::matches(secret_text, &installation.secret_hash) {
        return Ok(false);
    }
    installations::deactivate(&mut *conn, installation.id, now).await?;
    Ok(true)
}

/// Frees the seats of installations not seen for `release_after_days`.
pub async fn release_inactive(
    conn: &mut PgConnection,
    now: OffsetDateTime,
    settings: &LicenceSettings,
) -> Result<Vec<Released>, sqlx::Error> {
    let cutoff = now - Duration::days(i64::from(settings.release_after_days));
    installations::release_inactive(conn, cutoff, now).await
}
