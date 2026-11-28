//! The mails the API sends (D37): a mail of a kind, to an address, in a
//! language, with the data its template needs, behind one interface
//! ([`Mailer`]) so that the rest of the API never names a provider.
//!
//! - What happens queues a `mail` job (`queue`): the kind, the user or team
//!   member it goes to (an id, never an address), the language and record ids,
//!   at the time it is due, once per key (a licence, a kind and a period).
//! - The queue's worker runs it when it is due (`compose`): it reads the state
//!   now, skips a mail whose reason has gone (no "your trial ends" after a
//!   purchase), reads the address, builds the template's data, and hands the
//!   mail to the provider.
//! - Providers: `listmonk` (its transactional API, [`listmonk`]) and `log`
//!   (development and tests: the mail is logged, without its address). Which
//!   one is a deployment's setting (`DEVTAB_MAIL`); without one, `mail` jobs
//!   wait in the queue and nothing else changes.
//!
//! The texts are templates of the provider, not code: `mail/listmonk/` holds
//! a skeleton of every kind in every language ([`templates`]), which `admin
//! mail templates` uploads. Supabase Auth's own mails (sign-in, reset,
//! invite) stay Supabase's.

pub mod compose;
pub mod listmonk;
pub mod queue;
pub mod templates;

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde_json::{Map, Value};

/// Every kind of mail, as jobs, templates and settings name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MailKind {
    /// A trial has started: how long, what it gives, how to connect.
    TrialStarted,
    /// A trial ends in a few days (`DEVTAB_MAIL_TRIAL_REMIND_DAYS`).
    TrialEnding,
    /// A trial has ended without a purchase.
    TrialEnded,
    /// A paid licence was issued: welcome, and what the subscription gives.
    Welcome,
    /// A licence was added to an account that existed already (a purchase
    /// without signing in by an address that has an account).
    LicenceAdded,
    /// A yearly subscription renews soon (`DEVTAB_MAIL_RENEWAL_REMIND_DAYS`).
    RenewalReminder,
    /// A renewal could not be charged; the provider retries.
    PaymentFailed,
    /// The subscription was cancelled and ends at its period's end.
    SubscriptionCancelled,
    /// The subscription has ended.
    SubscriptionEnded,
    /// A team's owner invited this address.
    TeamInvite,
}

impl MailKind {
    pub const ALL: [Self; 10] = [
        Self::TrialStarted,
        Self::TrialEnding,
        Self::TrialEnded,
        Self::Welcome,
        Self::LicenceAdded,
        Self::RenewalReminder,
        Self::PaymentFailed,
        Self::SubscriptionCancelled,
        Self::SubscriptionEnded,
        Self::TeamInvite,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TrialStarted => "trial-started",
            Self::TrialEnding => "trial-ending",
            Self::TrialEnded => "trial-ended",
            Self::Welcome => "welcome",
            Self::LicenceAdded => "licence-added",
            Self::RenewalReminder => "renewal-reminder",
            Self::PaymentFailed => "payment-failed",
            Self::SubscriptionCancelled => "subscription-cancelled",
            Self::SubscriptionEnded => "subscription-ended",
            Self::TeamInvite => "team-invite",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

impl fmt::Display for MailKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The languages a mail is written in; the first is the fallback.
pub const LOCALES: [&str; 2] = ["en", "ru"];

/// A mail ready to send.
#[derive(Clone, PartialEq)]
pub struct Mail {
    pub kind: MailKind,
    /// The address, read when the mail is sent; never logged.
    pub to: String,
    /// One of [`LOCALES`].
    pub locale: String,
    /// What the template reads (`{{ .Tx.Data.<key> }}` in listmonk): strings,
    /// so that every provider takes them.
    pub data: Map<String, Value>,
}

// The address must never reach a log through `{:?}`.
impl fmt::Debug for Mail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mail")
            .field("kind", &self.kind)
            .field("locale", &self.locale)
            .field("data", &self.data)
            .finish_non_exhaustive()
    }
}

/// Why a mail was not sent.
#[derive(Debug, thiserror::Error)]
pub enum MailError {
    /// Worth trying again later (the provider could not be reached, limits).
    #[error("{0}")]
    Retry(String),
    /// Trying again cannot help (no template for the kind, a refusal).
    #[error("{0}")]
    Permanent(String),
}

/// What [`Mailer::send`] returns.
pub type MailFuture<'a> = Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>;

/// A provider that sends mails.
pub trait Mailer: Send + Sync + fmt::Debug {
    /// The provider's name, for the log.
    fn name(&self) -> &'static str;

    /// Sends `mail` once.
    fn send<'a>(&'a self, mail: &'a Mail) -> MailFuture<'a>;
}

/// The `log` provider: each mail is logged (its kind and language, never the
/// address or the data) and counts as sent. For development and tests;
/// refused in `prod`.
#[derive(Debug, Default)]
pub struct LogMailer;

impl Mailer for LogMailer {
    fn name(&self) -> &'static str {
        "log"
    }

    fn send<'a>(&'a self, mail: &'a Mail) -> MailFuture<'a> {
        Box::pin(async move {
            tracing::info!(kind = %mail.kind, locale = %mail.locale, "mail (log provider, not sent)");
            Ok(())
        })
    }
}

/// When the reminders go out, from the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailSchedule {
    /// Days before a trial ends (`DEVTAB_MAIL_TRIAL_REMIND_DAYS`, default 3
    /// and 1); one mail for each.
    pub trial_remind_days: Vec<u32>,
    /// Days before a yearly subscription renews
    /// (`DEVTAB_MAIL_RENEWAL_REMIND_DAYS`, default 7).
    pub renewal_remind_days: u32,
}

impl Default for MailSchedule {
    fn default() -> Self {
        Self {
            trial_remind_days: vec![3, 1],
            renewal_remind_days: 7,
        }
    }
}

/// Which provider sends the mails (`DEVTAB_MAIL`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MailSettings {
    /// None: `mail` jobs wait in the queue.
    #[default]
    Off,
    /// Logged, not sent.
    Log,
    /// listmonk's transactional API.
    Listmonk(listmonk::ListmonkSettings),
}

/// The provider of `settings`, its secrets read; none when off.
pub fn connect(settings: &MailSettings) -> anyhow::Result<Option<std::sync::Arc<dyn Mailer>>> {
    Ok(match settings {
        MailSettings::Off => None,
        MailSettings::Log => Some(std::sync::Arc::new(LogMailer)),
        MailSettings::Listmonk(listmonk) => Some(std::sync::Arc::new(
            listmonk::ListmonkMailer::connect(listmonk)?,
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_name_and_comes_back_from_it() {
        for kind in MailKind::ALL {
            assert_eq!(MailKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(MailKind::parse("spam"), None);
        let mail = Mail {
            kind: MailKind::Welcome,
            to: "person@example.invalid".to_owned(),
            locale: "en".to_owned(),
            data: Map::new(),
        };
        assert!(!format!("{mail:?}").contains("person@"));
    }
}
