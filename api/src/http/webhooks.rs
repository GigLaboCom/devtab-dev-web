//! `POST /api/webhooks/{provider}`: a payment provider's events, on the
//! public listener under the site's host. No CORS and no `Origin` check: the
//! signature is the proof.
//!
//! The raw body (up to 256 KB) is checked before anything reads it; a
//! mismatch answers 400 and only the provider is logged. A valid event is
//! stored once (a repeated delivery is answered 200 at once), a job to
//! process it is queued in the same transaction, and the answer is 200
//! before any processing, so that the provider never waits on it. A database
//! that cannot take the event answers an error, and the provider delivers it
//! again later.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

use super::error::AppError;
use crate::jobs::worker::PROCESS_EVENT;
use crate::payments::stripe::{self, webhook};
use crate::state::AppState;
use crate::store::{events, jobs};

/// The largest event body taken.
pub const BODY_LIMIT: usize = 256 * 1024;

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/api/webhooks/{provider}", post(receive))
        .with_state(state)
}

async fn receive(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let payments = state.payments.as_ref().ok_or(AppError::NotFound)?;
    if provider != stripe::ID {
        return Err(AppError::NotFound);
    }
    let settings = payments.stripe.settings();
    let now = state.clock.now();
    let header = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let event = webhook::verify(
        header,
        &body,
        &payments.webhook_secret,
        now,
        settings.webhook_tolerance_s,
        settings.mode,
    )
    .map_err(|err| {
        tracing::warn!(provider = %provider, reason = %err, "webhook refused");
        AppError::Signature
    })?;
    if event.api_version.as_deref() != Some(settings.api_version.as_str()) {
        // Stored and processed all the same: the adapter reads only ids,
        // whose places this version range keeps; the destination should be
        // recreated with the configured version.
        tracing::warn!(
            provider = %provider,
            event_version = event.api_version.as_deref().unwrap_or("-"),
            configured = %settings.api_version,
            "webhook rendered in another API version"
        );
    }
    let payload: Value = serde_json::from_slice(&body).map_err(|err| {
        AppError::Internal(anyhow::anyhow!("a verified body stopped parsing: {err}"))
    })?;
    let occurred_at = time::OffsetDateTime::from_unix_timestamp(event.created).unwrap_or(now);
    let mut tx = state.store.pool().begin().await?;
    let stored = events::insert(
        &mut *tx,
        &events::NewEvent {
            provider: stripe::ID,
            event_id: &event.id,
            event_type: &event.kind,
            occurred_at,
            received_at: now,
            payload: &payload,
        },
    )
    .await?;
    if let Some(id) = stored {
        jobs::enqueue(&mut *tx, PROCESS_EVENT, &json!({ "event": id }), now).await?;
    }
    tx.commit().await?;
    if stored.is_some() {
        state.wake.notify_one();
    }
    Ok(Json(json!({ "received": true })))
}
