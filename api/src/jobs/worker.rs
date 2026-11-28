//! The queue's worker: runs the jobs of `app.jobs` as they come due, one per
//! transaction, each in a savepoint so that a failed job leaves nothing but
//! the record of its failure. A webhook wakes it at once; otherwise it looks
//! every few seconds.
//!
//! A job that fails is tried again after 1, 5 and 30 minutes, then 2, 6 and
//! 24 hours; then, or at once when trying again cannot help, it is given up
//! and logged as an error (an alarm). The same holds for a mail: a provider's
//! refusal gives it up at once, an outage retries it.

use std::time::Duration as StdDuration;

use serde_json::Value;
use sqlx::{Connection as _, PgConnection};
use time::{Duration, OffsetDateTime};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::mail::{MailError, compose};
use crate::payments::process::{self, ProcessError};
use crate::state::AppState;
use crate::store::{events, jobs};

/// Processing a stored provider event: `{"event": <provider_events.id>}`.
pub const PROCESS_EVENT: &str = "process_event";

/// A mail of a kind to a user or a team member, in a language, with record
/// ids (`crate::mail::queue`); run only when a mailer is configured
/// (`DEVTAB_MAIL`), otherwise it waits in the queue.
pub use crate::mail::queue::MAIL;

/// The kinds of jobs this worker runs now; a job of any other kind waits.
pub fn runs(state: &AppState) -> Vec<&'static str> {
    let mut kinds = vec![PROCESS_EVENT];
    if state.mailer.is_some() {
        kinds.push(MAIL);
    }
    kinds
}

/// How often the worker looks for due jobs when nothing wakes it.
pub const POLL: StdDuration = StdDuration::from_secs(5);

/// The pauses before each retry of a failed job.
pub const RETRIES: [Duration; 6] = [
    Duration::minutes(1),
    Duration::minutes(5),
    Duration::minutes(30),
    Duration::hours(2),
    Duration::hours(6),
    Duration::hours(24),
];

/// At most this many jobs per run, so that a long queue does not starve the
/// stop signal.
const MAX_PER_RUN: usize = 100;

/// Runs the jobs due now, by the state's clock; how many ran.
pub async fn run_due(state: &AppState) -> Result<usize, sqlx::Error> {
    let mut ran = 0;
    let kinds = runs(state);
    while ran < MAX_PER_RUN {
        let now = state.clock.now();
        let mut tx = state.store.pool().begin().await?;
        let Some(job) = jobs::claim_due(&mut *tx, now, 1, &kinds)
            .await?
            .into_iter()
            .next()
        else {
            tx.rollback().await?;
            break;
        };
        run_one(state, &mut tx, &job, now).await?;
        tx.commit().await?;
        ran += 1;
    }
    Ok(ran)
}

async fn run_one(
    state: &AppState,
    conn: &mut PgConnection,
    job: &jobs::JobRow,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let mut savepoint = conn.begin().await?;
    let result = match job.kind.as_str() {
        PROCESS_EVENT => process_event(state, &mut savepoint, &job.payload, now).await,
        MAIL => send_mail(state, &mut savepoint, job, now).await,
        other => Err(ProcessError::Permanent(format!(
            "unknown job kind `{other}`"
        ))),
    };
    match result {
        Ok(()) => {
            savepoint.commit().await?;
            jobs::complete(&mut *conn, job.id, now).await
        }
        Err(err) => {
            savepoint.rollback().await?;
            let attempt = usize::try_from(job.attempts).unwrap_or(usize::MAX);
            let retry = match &err {
                ProcessError::Retry(_) => RETRIES.get(attempt).copied(),
                ProcessError::Permanent(_) => None,
            };
            let message = err.to_string();
            if let Some(pause) = retry {
                tracing::warn!(job = job.id, kind = %job.kind, attempt = attempt + 1, error = %message, "job failed, will retry");
                jobs::fail(&mut *conn, job.id, &message, now + pause).await?;
            } else {
                tracing::error!(job = job.id, kind = %job.kind, attempt = attempt + 1, error = %message, "job failed for good");
                jobs::give_up(&mut *conn, job.id, &message, now).await?;
            }
            if job.kind == PROCESS_EVENT
                && let Some(event) = job.payload.get("event").and_then(Value::as_i64)
            {
                events::fail(&mut *conn, event, &message, retry.is_none()).await?;
            }
            Ok(())
        }
    }
}

async fn process_event(
    state: &AppState,
    conn: &mut PgConnection,
    payload: &Value,
    now: OffsetDateTime,
) -> Result<(), ProcessError> {
    let id = payload
        .get("event")
        .and_then(Value::as_i64)
        .ok_or_else(|| ProcessError::Permanent("the job names no event".to_owned()))?;
    let payments = state
        .payments
        .as_ref()
        .ok_or_else(|| ProcessError::Retry("no payment provider is configured".to_owned()))?;
    let hooks = process::Hooks {
        accounts: state.accounts.as_deref(),
        schedule: &state.mail_schedule,
    };
    process::process_event(conn, &payments.stripe, &hooks, id, now).await?;
    Ok(())
}

/// Sends a due mail, or skips it when its reason has gone
/// (`crate::mail::compose`). Sent at least once: a mail sent just before the
/// transaction failed would go again at the retry.
async fn send_mail(
    state: &AppState,
    conn: &mut PgConnection,
    job: &jobs::JobRow,
    now: OffsetDateTime,
) -> Result<(), ProcessError> {
    let mailer = state
        .mailer
        .as_ref()
        .ok_or_else(|| ProcessError::Retry("no mailer is configured".to_owned()))?;
    let composed = compose::compose(conn, &job.payload, &state.catalogue.site_url, now)
        .await
        .map_err(mail_error)?;
    match composed {
        compose::Composed::Skip(reason) => {
            tracing::info!(job = job.id, reason, "mail skipped");
            Ok(())
        }
        compose::Composed::Send(mail) => {
            mailer.send(&mail).await.map_err(mail_error)?;
            tracing::info!(job = job.id, kind = %mail.kind, locale = %mail.locale, provider = mailer.name(), "mail sent");
            Ok(())
        }
    }
}

fn mail_error(err: MailError) -> ProcessError {
    match err {
        MailError::Retry(message) => ProcessError::Retry(message),
        MailError::Permanent(message) => ProcessError::Permanent(message),
    }
}

/// Runs [`run_due`] whenever the state's wake-up is notified, every
/// [`POLL`] otherwise, until `stop` changes.
pub fn spawn(state: AppState, mut stop: watch::Receiver<()>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(err) = run_due(&state).await {
                tracing::warn!(error = %err, "running due jobs failed");
            }
            tokio::select! {
                () = state.wake.notified() => {}
                () = tokio::time::sleep(POLL) => {}
                _ = stop.changed() => break,
            }
        }
    })
}
