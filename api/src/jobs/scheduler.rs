//! The periodic tasks: one tick an hour, the first right after start.
//!
//! A task runs in a transaction that first takes its advisory lock with
//! `pg_try_advisory_xact_lock`: whoever does not get it skips this tick, and
//! the lock goes away with the transaction even if the process dies.

use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::RELEASE_INACTIVE_LOCK;
use crate::licence::seats;
use crate::state::AppState;
use crate::store::installations::Released;

/// How often the scheduler wakes up.
pub const TICK: Duration = Duration::from_secs(60 * 60);

/// What one run of a task did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    Done(Vec<Released>),
    /// Another process holds the task's lock.
    Skipped,
}

/// Releases the seats of installations not seen for `release_after_days`
/// days, by the state's clock, under the task's advisory lock.
pub async fn release_inactive(state: &AppState) -> Result<Run, sqlx::Error> {
    let mut tx = state.store.pool().begin().await?;
    let locked = sqlx::query_scalar!(
        r#"select pg_try_advisory_xact_lock($1) as "locked!""#,
        RELEASE_INACTIVE_LOCK,
    )
    .fetch_one(&mut *tx)
    .await?;
    if !locked {
        tx.rollback().await?;
        return Ok(Run::Skipped);
    }
    let released = seats::release_inactive(&mut tx, state.clock.now(), &state.licence).await?;
    tx.commit().await?;
    Ok(Run::Done(released))
}

/// One tick: every task, each failure logged and left for the next tick.
pub async fn tick(state: &AppState) {
    match release_inactive(state).await {
        Ok(Run::Done(released)) if !released.is_empty() => {
            tracing::info!(released = released.len(), "released inactive installations");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "releasing inactive installations failed"),
    }
    state.limits.housekeeping();
}

/// Runs [`tick`] every [`TICK`] until `stop` changes.
pub fn spawn(state: AppState, mut stop: watch::Receiver<()>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => tick(&state).await,
                _ = stop.changed() => break,
            }
        }
    })
}
