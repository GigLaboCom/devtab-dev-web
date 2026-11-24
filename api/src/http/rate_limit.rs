//! Rate limits, in memory (one API instance). Limits keyed by the client's
//! address run before the body is read, so malformed requests count too;
//! limits keyed by the body (a key's hash, an installation) run after it is
//! parsed. Exceeding one answers 429 `rate-limited` with `Retry-After` in
//! whole seconds. The quotas come from the configuration.

use std::hash::Hash;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use governor::clock::{Clock as _, DefaultClock};
use governor::{DefaultKeyedRateLimiter, Quota};

use super::client_ip::ClientIp;
use super::error::AppError;
use crate::config::RateLimits;

pub type Limiter<K> = DefaultKeyedRateLimiter<K>;

/// An installation: `lid` and `inst`.
pub type InstallationKey = (String, String);

/// The limiters of the extension's endpoints.
#[derive(Debug)]
pub struct Limits {
    pub activate_per_ip: Arc<Limiter<IpAddr>>,
    pub activate_per_key: Limiter<[u8; 32]>,
    pub refresh_per_ip: Arc<Limiter<IpAddr>>,
    pub refresh_per_installation: Limiter<InstallationKey>,
    pub deactivate_per_installation: Limiter<InstallationKey>,
}

fn per_hour<K: Hash + Eq + Clone>(n: NonZeroU32) -> Limiter<K> {
    DefaultKeyedRateLimiter::keyed(Quota::per_hour(n))
}

impl Limits {
    pub fn new(quotas: &RateLimits) -> Self {
        Self {
            activate_per_ip: Arc::new(per_hour(quotas.activate_per_ip)),
            activate_per_key: per_hour(quotas.activate_per_key),
            refresh_per_ip: Arc::new(per_hour(quotas.refresh_per_ip)),
            refresh_per_installation: per_hour(quotas.refresh_per_installation),
            deactivate_per_installation: per_hour(quotas.deactivate_per_installation),
        }
    }

    /// Drops keys that are back to a full quota, so the maps do not grow
    /// forever. The scheduler calls it periodically.
    pub fn housekeeping(&self) {
        self.activate_per_ip.retain_recent();
        self.activate_per_key.retain_recent();
        self.refresh_per_ip.retain_recent();
        self.refresh_per_installation.retain_recent();
        self.deactivate_per_installation.retain_recent();
    }
}

/// One request against `key`'s quota: `rate-limited` with the seconds until
/// the next request is allowed (at least 1) when it is used up.
pub fn check<K: Hash + Eq + Clone>(limiter: &Limiter<K>, key: &K) -> Result<(), AppError> {
    limiter.check_key(key).map_err(|not_until| {
        let wait = not_until.wait_time_from(DefaultClock::default().now());
        let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        AppError::RateLimited {
            retry_after_s: seconds.max(1),
        }
    })
}

/// Middleware: the client address's quota, before the body is read.
pub async fn per_ip(
    State(limiter): State<Arc<Limiter<IpAddr>>>,
    client: ClientIp,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    check(&limiter, &client.0)?;
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quota_runs_out_and_says_when_to_come_back() {
        let limiter: Limiter<u8> = per_hour(NonZeroU32::new(2).unwrap());
        assert!(check(&limiter, &1).is_ok());
        assert!(check(&limiter, &1).is_ok());
        match check(&limiter, &1) {
            Err(AppError::RateLimited { retry_after_s }) => {
                // Two per hour: one more after half an hour.
                assert!((1_790..=1_800).contains(&retry_after_s), "{retry_after_s}");
            }
            other => panic!("{other:?}"),
        }
        assert!(check(&limiter, &2).is_ok(), "another key has its own quota");
    }
}
