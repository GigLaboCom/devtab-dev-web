//! Shared state handed to every handler.

use std::sync::Arc;

use tokio::sync::Notify;

use crate::auth::{UserCache, Verifier};
use crate::clock::SharedClock;
use crate::config::Config;
use crate::http::rate_limit::Limits;
use crate::licence::LicenceSettings;
use crate::licence::signer::Signer;
use crate::payments::Payments;
use crate::plans::catalogue::CatalogueSettings;
use crate::store::Store;

#[derive(Debug, Clone)]
pub struct AppState {
    pub store: Store,
    pub clock: SharedClock,
    pub signer: Arc<Signer>,
    pub limits: Arc<Limits>,
    pub licence: LicenceSettings,
    pub catalogue: Arc<CatalogueSettings>,
    /// Checks the access tokens of signed-in people; none without
    /// `DEVTAB_SUPABASE_URL`, and then a route that needs one answers 503.
    pub auth: Option<Arc<Verifier>>,
    /// Recently loaded `app.users` rows.
    pub users: UserCache,
    /// The payment provider; none: no checkout starts and no webhook is
    /// taken.
    pub payments: Option<Arc<Payments>>,
    /// Wakes the queue's worker when a job is added.
    pub wake: Arc<Notify>,
}

impl AppState {
    pub fn new(store: Store, clock: SharedClock, signer: Signer, config: &Config) -> Self {
        Self {
            store,
            clock,
            signer: Arc::new(signer),
            limits: Arc::new(Limits::new(&config.rate_limits)),
            licence: config.licence,
            catalogue: Arc::new(config.catalogue()),
            auth: None,
            users: UserCache::default(),
            payments: None,
            wake: Arc::new(Notify::new()),
        }
    }

    /// The same state, taking payments with `payments`.
    #[must_use]
    pub fn with_payments(mut self, payments: Payments) -> Self {
        self.payments = Some(Arc::new(payments));
        self
    }

    /// The same state, checking access tokens with `verifier`.
    #[must_use]
    pub fn with_auth(mut self, verifier: Verifier) -> Self {
        self.auth = Some(Arc::new(verifier));
        self
    }
}
