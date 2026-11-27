//! Background work of the API process: the queue's worker (`worker`), which
//! runs the jobs of `app.jobs`, and the periodic tasks (`scheduler`).
//!
//! The scheduler runs each periodic task under a Postgres advisory lock, so
//! that two API processes (during a deploy, say) never run the same task at
//! once: releasing the seats of installations not seen for
//! `DEVTAB_RELEASE_AFTER_DAYS` days, and tidying the rate limiters' memory.

pub mod scheduler;
pub mod worker;

/// The advisory lock of the task that releases inactive installations
/// ("devtab" in ASCII, then the task's number).
pub const RELEASE_INACTIVE_LOCK: i64 = 0x6465_7674_6162_0001;
