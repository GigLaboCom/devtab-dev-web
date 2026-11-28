//! Shared state handed to every handler.

use std::sync::Arc;

use tokio::sync::Notify;

use crate::accounts::Accounts;
use crate::auth::{UserCache, Verifier};
use crate::clock::SharedClock;
use crate::config::Config;
use crate::http::rate_limit::Limits;
use crate::licence::LicenceSettings;
use crate::licence::signer::Signer;
use crate::mail::{MailSchedule, Mailer};
use crate::payments::Payments;
use crate::plans::catalogue::CatalogueSettings;
use crate::store::Store;
use crate::trial::TrialSettings;

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
    /// Whether a checkout may start without a signed-in person
    /// (`DEVTAB_GUEST_CHECKOUT`).
    pub guest_checkout: bool,
    /// Supabase Auth's admin API, which invites the buyer of a guest
    /// checkout; none without the project's secret key, and then a guest
    /// checkout is refused (and one already paid waits for it).
    pub accounts: Option<Arc<Accounts>>,
    /// Who sends the mails; none: `mail` jobs wait in the queue.
    pub mailer: Option<Arc<dyn Mailer>>,
    /// When the reminders go out.
    pub mail_schedule: Arc<MailSchedule>,
    /// The trial without a card; none: not offered.
    pub trial: Option<Arc<TrialSettings>>,
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
            guest_checkout: config.guest_checkout,
            accounts: None,
            mailer: None,
            mail_schedule: Arc::new(config.mail_schedule.clone()),
            trial: config.trial.clone().map(Arc::new),
        }
    }

    /// The same state, sending mails with `mailer`.
    #[must_use]
    pub fn with_mailer(mut self, mailer: Arc<dyn Mailer>) -> Self {
        self.mailer = Some(mailer);
        self
    }

    /// The same state, inviting the buyers of guest checkouts with `accounts`.
    #[must_use]
    pub fn with_accounts(mut self, accounts: Accounts) -> Self {
        self.accounts = Some(Arc::new(accounts));
        self
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
