//! Queuing a mail: a `mail` job with the kind, the recipient (a user or a
//! team member, by id), the language and the record ids its template needs,
//! due at `run_at`, once per key. The address is read when the mail is sent,
//! never kept in the queue.
//!
//! The keys make each mail once only, however many events ask for it: a
//! licence and a kind (`welcome:<licence>`), and the period it is about when a
//! kind can come again (`payment-failed:<licence>:<overdue since>`,
//! `renewal-reminder:<licence>:<period end>`, `trial-ending:<licence>:<days>`).

use serde_json::{Value, json};
use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{LOCALES, MailKind};
use crate::store::jobs;

/// The job kind of a mail.
pub const MAIL: &str = "mail";

/// Who a mail goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum To {
    /// A user of the site: their account's address.
    User(Uuid),
    /// A team member (`app.licence_members.id`): the address the owner
    /// invited.
    Member(Uuid),
}

/// Queues `kind` to `to` in `locale` with `data`, due at `run_at`, unless a
/// mail with `key` was ever queued. True when this one was added.
pub async fn queue(
    db: impl PgExecutor<'_>,
    kind: MailKind,
    to: To,
    locale: Option<&str>,
    data: Value,
    run_at: OffsetDateTime,
    key: &str,
) -> Result<bool, sqlx::Error> {
    let locale = locale.filter(|l| LOCALES.contains(l)).unwrap_or(LOCALES[0]);
    let mut payload = json!({ "mail": kind.as_str(), "locale": locale, "data": data });
    match to {
        To::User(id) => payload["user"] = json!(id),
        To::Member(id) => payload["member"] = json!(id),
    }
    jobs::enqueue_once(db, MAIL, &payload, run_at, &format!("mail:{kind}:{key}")).await
}
