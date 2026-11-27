//! The signed-in person behind a request to the internal listener, as an
//! extractor: `Authorization: Bearer <Supabase access token>`, checked by the
//! [`Verifier`](super::token::Verifier), then the person's `app.users` row
//! (kept for [`USER_TTL`]). The person is only ever the token's `sub`: never
//! an id from the body, the query or another header, and never a role from
//! the token.
//!
//! No header, or a token that does not pass: 401 with
//! `WWW-Authenticate: Bearer`. A blocked account: 403. The project's keys
//! out of reach, or sign-in not configured: 503.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::token::TokenError;
use crate::http::error::AppError;
use crate::state::AppState;
use crate::store::users;

/// How long a loaded `app.users` row is used again.
pub const USER_TTL: Duration = Duration::seconds(60);
/// With this many people kept, the expired ones are dropped first.
pub const USER_CACHE_CAP: usize = 10_000;

/// The signed-in person of this request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentUser {
    pub id: Uuid,
}

/// Whether each recently seen person is blocked, by id, with when it was read.
#[derive(Debug, Clone, Default)]
pub struct UserCache {
    inner: Arc<Mutex<HashMap<Uuid, (OffsetDateTime, bool)>>>,
}

impl UserCache {
    fn get(&self, id: Uuid, now: OffsetDateTime) -> Option<bool> {
        let map = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        map.get(&id)
            .filter(|(at, _)| now - *at < USER_TTL)
            .map(|(_, blocked)| *blocked)
    }

    fn put(&self, id: Uuid, blocked: bool, now: OffsetDateTime) {
        let mut map = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if map.len() >= USER_CACHE_CAP {
            map.retain(|_, (at, _)| now - *at < USER_TTL);
        }
        if map.len() < USER_CACHE_CAP {
            map.insert(id, (now, blocked));
        }
    }

    /// Forgets a person, after their status changed.
    pub fn forget(&self, id: Uuid) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
}

/// The token of `Authorization: Bearer <token>`; `None` without the header.
fn bearer(parts: &Parts) -> Result<Option<&str>, AppError> {
    let Some(value) = parts.headers.get(AUTHORIZATION) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| AppError::Unauthorized)?;
    let (scheme, token) = value.split_once(' ').ok_or(AppError::Unauthorized)?;
    let token = token.trim();
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return Err(AppError::Unauthorized);
    }
    Ok(Some(token))
}

/// Whether the person is blocked; `None` when `app.users` has no row.
async fn blocked(state: &AppState, id: Uuid) -> Result<Option<bool>, AppError> {
    let now = state.clock.now();
    if let Some(blocked) = state.users.get(id, now) {
        return Ok(Some(blocked));
    }
    let Some(row) = users::find(state.store.pool(), id).await? else {
        return Ok(None);
    };
    let blocked = row.status == "blocked";
    state.users.put(id, blocked, now);
    Ok(Some(blocked))
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts)?.ok_or(AppError::Unauthorized)?;
        let Some(verifier) = state.auth.as_deref() else {
            tracing::warn!("a signed-in request arrived, but DEVTAB_SUPABASE_URL is not set");
            return Err(AppError::Unavailable);
        };
        let claims = verifier.verify(token).await.map_err(|err| match err {
            TokenError::KeysUnavailable(_) => {
                tracing::warn!(error = %err, "a token could not be checked");
                AppError::Unavailable
            }
            other => {
                tracing::info!(reason = %other, "a token was refused");
                AppError::Unauthorized
            }
        })?;
        match blocked(state, claims.sub).await? {
            None => {
                tracing::warn!(user = %claims.sub, "a token that passed, for a person without an app.users row");
                Err(AppError::Unauthorized)
            }
            Some(true) => Err(AppError::Forbidden("This account is blocked.".to_owned())),
            Some(false) => Ok(Self { id: claims.sub }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(value: Option<&str>) -> Parts {
        let mut request = axum::http::Request::builder();
        if let Some(value) = value {
            request = request.header(AUTHORIZATION, value);
        }
        request.body(()).unwrap().into_parts().0
    }

    #[test]
    fn the_bearer_header() {
        assert_eq!(bearer(&parts(None)).unwrap(), None);
        assert_eq!(bearer(&parts(Some("Bearer abc"))).unwrap(), Some("abc"));
        assert_eq!(bearer(&parts(Some("bearer  abc "))).unwrap(), Some("abc"));
        for bad in ["Basic abc", "Bearer ", "abc"] {
            assert!(bearer(&parts(Some(bad))).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_cache_keeps_a_person_for_a_minute_and_forgets_on_demand() {
        let cache = UserCache::default();
        let id = Uuid::from_u128(7);
        let now = OffsetDateTime::from_unix_timestamp(1_795_392_000).unwrap();
        cache.put(id, true, now);
        assert_eq!(cache.get(id, now + Duration::seconds(59)), Some(true));
        assert_eq!(cache.get(id, now + USER_TTL), None);
        cache.forget(id);
        assert_eq!(cache.get(id, now), None);
    }
}
