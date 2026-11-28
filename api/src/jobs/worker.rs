//! The queue's worker: runs the jobs of `app.jobs` as they come due, one per
//! transaction, each in a savepoint so that a failed job leaves nothing but
//! the record of its failure. A webhook wakes it at once; otherwise it looks
//! every few seconds.
//!
//! A job that fails is tried again after 1, 5 and 30 minutes, then 2, 6 and
//! 24 hours; then, or at once when trying again cannot help, it is given up
//! and logged as an error (an alarm).

use std::time::Duration as StdDuration;

use serde_json::Value;
use sqlx::{Connection as _, PgConnection};
use time::{Duration, OffsetDateTime};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::payments::process::{self, ProcessError};
use crate::state::AppState;
use crate::store::{events, jobs};

/// Processing a stored provider event: `{"event": <provider_events.id>}`.
pub const PROCESS_EVENT: &str = "process_event";

/// A mail of one kind to one user, in a language, with the data its
/// template needs: `{"mail": <kind>, "user": <users.id>, "locale": "en"|"ru",
/// "data": {…record ids…}}`. The address is read when it is sent (never kept
/// in the queue), and the mailer (a provider behind one interface, the
/// listmonk transactional API first) picks the template by kind and locale.
/// Queued, not run yet: this worker does not claim it ([`RUNS`]) until the
/// mailer exists, so these jobs wait in `app.jobs` for it.
pub const MAIL: &str = "mail";

/// The mail kinds queued so far, as [`MAIL`] jobs name them.
pub mod mails {
    /// A licence was added to an account that existed already (a guest
    /// purchase by an address that has an account, which gets no invite):
    /// data `{"licence": <licences.id>, "checkout": <checkouts.id>, "plan": <plan id>}`.
    pub const LICENCE_ADDED: &str = "licence-added";
}

/// The kinds of jobs this worker runs; a job of any other kind waits.
pub const RUNS: [&str; 1] = [PROCESS_EVENT];

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
    while ran < MAX_PER_RUN {
        let now = state.clock.now();
        let mut tx = state.store.pool().begin().await?;
        let Some(job) = jobs::claim_due(&mut *tx, now, 1, &RUNS)
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
    process::process_event(conn, &payments.stripe, state.accounts.as_deref(), id, now).await?;
    Ok(())
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
