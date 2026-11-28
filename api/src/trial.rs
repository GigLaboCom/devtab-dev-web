//! The trial without a card (D36): a signed-in person takes, once per inbox,
//! a licence of the trial's plan for `DEVTAB_TRIAL_DAYS` days (7 to 14).
//!
//! A trial is an ordinary licence of kind `trial`, not a parallel path: the
//! plan's seats, a key, `status` `active`, and its end in `paid_until`. The
//! token reads it as a complimentary licence with an end (`paid` is the end,
//! no `term`, so no payment grace, `renews: false`; `licence::entitlement`),
//! so the extension activates, refreshes and expires it as it does any
//! licence: past `paid_until` the token says `ended`, activation answers
//! `licence-inactive`, and connecting from the account skips it.
//!
//! - Signed in only: the account's address is confirmed (Supabase Auth
//!   confirms an address before a password signs in; the mail's link and code
//!   and Google prove it too).
//! - Once per canonical inbox: the SHA-256 of `app.users.email_canonical` goes
//!   into `app.trial_inboxes`, which outlives the account; another spelling
//!   of the same inbox, or a new account after deleting the old one, is
//!   refused (`trial-used`). The address itself is not kept there.
//! - Not after a purchase: a person who ever had a paid licence is refused
//!   (`trial-not-eligible`).
//! - A purchase ends a running trial (`payments::process`): the paid licence
//!   replaces it, and the trial's reminders are skipped when they come due.
//!
//! Starting one queues its mails: trial-started now, trial-ending
//! `DEVTAB_MAIL_TRIAL_REMIND_DAYS` before its end, trial-ended at its end.

use serde_json::json;
use sha2::{Digest as _, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::http::error::AppError;
use crate::licence::key;
use crate::mail::queue::{self, To};
use crate::mail::{MailKind, MailSchedule};
use crate::store::licences::{self, NewLicence};
use crate::store::{plans, users};

/// The trial's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialSettings {
    /// `DEVTAB_TRIAL_DAYS`: 7 to 14.
    pub days: u32,
    /// `DEVTAB_TRIAL_PLAN`: the plan whose seats a trial gives.
    pub plan: String,
}

/// A started trial: its licence, its key (shown once), its end.
pub struct StartedTrial {
    pub lid: String,
    pub key: String,
    pub ends_at: OffsetDateTime,
}

// The key must never reach a log through `{:?}`.
impl std::fmt::Debug for StartedTrial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartedTrial")
            .field("lid", &self.lid)
            .field("ends_at", &self.ends_at)
            .finish_non_exhaustive()
    }
}

/// The SHA-256 of a canonical inbox: what `app.trial_inboxes` keeps.
pub fn inbox_hash(canonical: &str) -> [u8; 32] {
    Sha256::digest(canonical.as_bytes()).into()
}

/// Starts `user`'s trial in the caller's transaction.
pub async fn start(
    conn: &mut sqlx::PgConnection,
    settings: &TrialSettings,
    schedule: &MailSchedule,
    user: Uuid,
    locale: &str,
    now: OffsetDateTime,
) -> Result<StartedTrial, AppError> {
    let canonical = users::canonical_of(&mut *conn, user)
        .await?
        .ok_or(AppError::TrialNotEligible)?;
    let hash = inbox_hash(&canonical);
    // One inbox at a time, so that two accounts of one inbox cannot both pass.
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1, 0))",
        format!("trial:{canonical}"),
    )
    .execute(&mut *conn)
    .await?;
    if licences::ever_paid(&mut *conn, user).await? {
        return Err(AppError::TrialNotEligible);
    }
    let taken = sqlx::query_scalar!(
        r#"select exists (select 1 from app.trial_inboxes where inbox_hash = $1) as "exists!""#,
        hash.as_slice(),
    )
    .fetch_one(&mut *conn)
    .await?;
    if taken {
        return Err(AppError::TrialUsed);
    }
    let plan = plans::find(&mut *conn, &settings.plan)
        .await?
        .ok_or_else(|| anyhow::anyhow!("DEVTAB_TRIAL_PLAN names no plan `{}`", settings.plan))?;
    let seats = plan
        .seats
        .ok_or_else(|| anyhow::anyhow!("the trial's plan `{}` has no seats", plan.id))?;
    let ends_at = now + Duration::days(i64::from(settings.days));
    let lid = key::new_lid()?;
    let new_key = key::generate()?;
    let id = licences::insert(
        &mut *conn,
        &NewLicence {
            lid: &lid,
            user_id: Some(user),
            customer_id: None,
            plan_id: &plan.id,
            kind: "trial",
            seats,
            members: 1,
            paid_until: Some(ends_at),
            // A trial never renews: the token's rules read `kind`, not this.
            cancel_at_period_end: false,
            provider: None,
            provider_subscription_id: None,
            key_hash: &new_key.hash,
            key_hint: &new_key.hint,
            note: None,
        },
    )
    .await?;
    licences::set_locale(&mut *conn, id, locale).await?;
    sqlx::query!(
        "insert into app.trial_inboxes (inbox_hash, licence_id, started_at) values ($1, $2, $3)",
        hash.as_slice(),
        id,
        now,
    )
    .execute(&mut *conn)
    .await?;

    queue_mails(conn, schedule, user, id, locale, ends_at, now).await?;
    tracing::info!(lid = %lid, plan = %plan.id, days = settings.days, "trial started");
    Ok(StartedTrial {
        lid,
        key: new_key.display,
        ends_at,
    })
}

/// The trial's mails: started now, the reminders before its end, ended at
/// its end.
async fn queue_mails(
    conn: &mut sqlx::PgConnection,
    schedule: &MailSchedule,
    user: Uuid,
    id: Uuid,
    locale: &str,
    ends_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let data = json!({ "licence": id });
    let to = To::User(user);
    queue::queue(
        &mut *conn,
        MailKind::TrialStarted,
        to,
        Some(locale),
        data.clone(),
        now,
        &id.to_string(),
    )
    .await?;
    for days in &schedule.trial_remind_days {
        let at = ends_at - Duration::days(i64::from(*days));
        if at > now {
            queue::queue(
                &mut *conn,
                MailKind::TrialEnding,
                to,
                Some(locale),
                data.clone(),
                at,
                &format!("{id}:{days}"),
            )
            .await?;
        }
    }
    queue::queue(
        &mut *conn,
        MailKind::TrialEnded,
        to,
        Some(locale),
        data,
        ends_at,
        &id.to_string(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inbox_is_kept_as_a_hash() {
        let hash = inbox_hash("firstlast@gmail.com");
        assert_eq!(hash, inbox_hash("firstlast@gmail.com"));
        assert_ne!(hash, inbox_hash("first.last@gmail.com"));
    }
}
