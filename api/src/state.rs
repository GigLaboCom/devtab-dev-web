//! Shared state handed to every handler.

use std::sync::Arc;

use crate::clock::SharedClock;
use crate::config::Config;
use crate::http::rate_limit::Limits;
use crate::licence::LicenceSettings;
use crate::licence::signer::Signer;
use crate::store::Store;

#[derive(Debug, Clone)]
pub struct AppState {
    pub store: Store,
    pub clock: SharedClock,
    pub signer: Arc<Signer>,
    pub limits: Arc<Limits>,
    pub licence: LicenceSettings,
}

impl AppState {
    pub fn new(store: Store, clock: SharedClock, signer: Signer, config: &Config) -> Self {
        Self {
            store,
            clock,
            signer: Arc::new(signer),
            limits: Arc::new(Limits::new(&config.rate_limits)),
            licence: config.licence,
        }
    }
}
