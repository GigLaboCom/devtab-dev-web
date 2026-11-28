//! A queued mail, when it is due: is it still to be sent, to which address,
//! and with what data. The state is read now, not when the mail was queued,
//! so that a mail whose reason has gone is skipped: no "your trial ends"
//! after a purchase ended the trial, no "payment failed" once it was paid, no
//! renewal reminder for a period that moved or a subscription cancelled since.
//!
//! The data are strings, the same for every kind where they apply, so that a
//! template may use any of them: `site_url`, `plans_url`, `connect_url`,
//! `sign_in_url`, `plan_name`, `key_hint`, `members`, `ends_on` (the end of
//! the paid period or the trial, `YYYY-MM-DD`), `days_left`.

use serde_json::{Map, Value, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{LOCALES, Mail, MailError, MailKind};
use crate::store::licences::MailLicence;
use crate::store::{licences, members, plans, users};

/// What to do with a due mail.
#[derive(Debug)]
pub enum Composed {
    Send(Mail),
    /// Its reason has gone, or there is nobody to send it to: done, unsent.
    Skip(&'static str),
}

impl From<sqlx::Error> for MailError {
    fn from(err: sqlx::Error) -> Self {
        Self::Retry(format!("database: {err}"))
    }
}

fn uuid_at(value: &Value, path: &[&str]) -> Option<Uuid> {
    let mut at = value;
    for key in path {
        at = at.get(key)?;
    }
    at.as_str().and_then(|s| Uuid::parse_str(s).ok())
}

/// The mail a `mail` job's `payload` asks for, as things stand at `now`.
pub async fn compose(
    conn: &mut PgConnection,
    payload: &Value,
    site_url: &str,
    now: OffsetDateTime,
) -> Result<Composed, MailError> {
    let kind = payload
        .get("mail")
        .and_then(Value::as_str)
        .and_then(MailKind::parse)
        .ok_or_else(|| MailError::Permanent("the job names no kind of mail".to_owned()))?;
    let locale = payload
        .get("locale")
        .and_then(Value::as_str)
        .filter(|l| LOCALES.contains(l))
        .unwrap_or(LOCALES[0])
        .to_owned();

    // The recipient.
    let (to, licence_id) = if let Some(user) = uuid_at(payload, &["user"]) {
        match users::find(&mut *conn, user).await? {
            None => return Ok(Composed::Skip("the account is gone")),
            Some(row) if row.status != "active" => {
                return Ok(Composed::Skip("the account is blocked"));
            }
            Some(_) => {}
        }
        let Some(email) = users::email_of(&mut *conn, user).await? else {
            return Ok(Composed::Skip("the account has no address"));
        };
        (email, uuid_at(payload, &["data", "licence"]))
    } else if let Some(member) = uuid_at(payload, &["member"]) {
        let Some((licence, email)) = members::find_by_id(&mut *conn, member).await? else {
            return Ok(Composed::Skip("the member was removed"));
        };
        (email, Some(licence))
    } else {
        return Err(MailError::Permanent(
            "the job names no recipient".to_owned(),
        ));
    };

    let Some(licence_id) = licence_id else {
        return Err(MailError::Permanent(format!(
            "a {kind} mail names no licence"
        )));
    };
    let Some(licence) = licences::find_for_mail(&mut *conn, licence_id).await? else {
        return Ok(Composed::Skip("the licence is gone"));
    };
    if let Some(reason) = gone(conn, kind, &licence, payload, now).await? {
        return Ok(Composed::Skip(reason));
    }

    let data = data_of(conn, &licence, &locale, site_url, now).await?;
    Ok(Composed::Send(Mail {
        kind,
        to,
        locale,
        data,
    }))
}

/// Why the mail is no longer to be sent, if it is not.
async fn gone(
    conn: &mut PgConnection,
    kind: MailKind,
    licence: &MailLicence,
    payload: &Value,
    now: OffsetDateTime,
) -> Result<Option<&'static str>, MailError> {
    let active = licence.status == "active";
    let trial = licence.kind == "trial";
    let bought = match licence.user_id {
        Some(user) => licences::has_active_paid(&mut *conn, user).await?,
        None => false,
    };
    let ends = licence.paid_until;
    Ok(match kind {
        MailKind::TrialStarted if !(trial && active) => Some("the trial is over"),
        MailKind::TrialEnding if !(trial && active) || bought => {
            Some("the trial is over or replaced")
        }
        MailKind::TrialEnding if ends.is_none_or(|end| end <= now) => Some("the trial has ended"),
        // Ended by a purchase (`ended`), it is not "ended without one".
        MailKind::TrialEnded if !(trial && active) || bought => Some("the trial was replaced"),
        MailKind::Welcome | MailKind::LicenceAdded | MailKind::TeamInvite if !active => {
            Some("the licence is not active")
        }
        MailKind::RenewalReminder => {
            let period = payload
                .get("data")
                .and_then(|d| d.get("period_end"))
                .and_then(Value::as_i64);
            let same_period = ends.map(OffsetDateTime::unix_timestamp) == period;
            (!(active
                && same_period
                && !licence.cancel_at_period_end
                && ends.is_some_and(|e| e > now)))
            .then_some("the period moved, or the subscription will not renew")
        }
        MailKind::PaymentFailed if !(active && licence.past_due_since.is_some()) => {
            Some("the payment went through")
        }
        MailKind::SubscriptionCancelled if !(active && licence.cancel_at_period_end) => {
            Some("the subscription renews again")
        }
        MailKind::SubscriptionEnded if licence.status != "ended" => {
            Some("the subscription runs again")
        }
        _ => None,
    })
}

/// The template's data for `licence`.
async fn data_of(
    conn: &mut PgConnection,
    licence: &MailLicence,
    locale: &str,
    site_url: &str,
    now: OffsetDateTime,
) -> Result<Map<String, Value>, MailError> {
    let prefix = if locale == LOCALES[0] {
        String::new()
    } else {
        format!("/{locale}")
    };
    let plan_name = plans::name_in(&mut *conn, &licence.plan_id, locale)
        .await?
        .unwrap_or_else(|| licence.plan_id.clone());
    let mut data = Map::new();
    let mut put = |key: &str, value: String| {
        data.insert(key.to_owned(), json!(value));
    };
    put("site_url", site_url.to_owned());
    put("plans_url", format!("{site_url}{prefix}/pro/"));
    put("connect_url", format!("{site_url}{prefix}/pro/connect/"));
    put("sign_in_url", format!("{site_url}{prefix}/sign-in/"));
    put("plan_name", plan_name);
    put("key_hint", licence.key_hint.clone());
    put("members", licence.members.to_string());
    if let Some(end) = licence.paid_until {
        put(
            "ends_on",
            format!(
                "{:04}-{:02}-{:02}",
                end.year(),
                u8::from(end.month()),
                end.day()
            ),
        );
        let left = (end - now).whole_hours().max(0);
        put("days_left", ((left + 23) / 24).to_string());
    }
    Ok(data)
}
