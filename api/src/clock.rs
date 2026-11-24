//! The time that decisions depend on. Logic takes a [`Clock`] instead of
//! reading the system time, so that tests can move time without sleeping.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime, UtcOffset};

/// An RFC 3339 time, in UTC.
pub fn parse_rfc3339(text: &str) -> Result<OffsetDateTime, time::error::Parse> {
    OffsetDateTime::parse(text, &Rfc3339).map(|at| at.to_offset(UtcOffset::UTC))
}

/// `2026-11-24T00:00:00Z`: RFC 3339 in UTC, whole seconds.
pub fn rfc3339(at: OffsetDateTime) -> String {
    let at = at.to_offset(UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

pub trait Clock: Send + Sync + fmt::Debug {
    fn now(&self) -> OffsetDateTime;
}

/// A clock shared by the handlers, the scheduler and the admin commands.
pub type SharedClock = Arc<dyn Clock>;

/// The system's time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// A clock that stands still until it is set or advanced.
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<Mutex<OffsetDateTime>>);

impl ManualClock {
    pub fn new(at: OffsetDateTime) -> Self {
        Self(Arc::new(Mutex::new(at)))
    }

    pub fn set(&self, at: OffsetDateTime) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = at;
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        *now += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> OffsetDateTime {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_clock_moves_only_when_told() {
        let start = OffsetDateTime::from_unix_timestamp(1_795_392_000).unwrap();
        let clock = ManualClock::new(start);
        assert_eq!(clock.now(), start);
        clock.advance(Duration::days(61));
        assert_eq!(clock.now(), start + Duration::days(61));
        clock.set(start);
        assert_eq!(clock.now(), start);
    }

    #[test]
    fn rfc3339_in_utc_and_whole_seconds() {
        let at = parse_rfc3339("2026-11-23T02:00:00.75+02:00").unwrap();
        assert_eq!(at.unix_timestamp(), 1_795_392_000);
        assert_eq!(rfc3339(at), "2026-11-23T00:00:00Z");
        assert!(parse_rfc3339("2026-11-23").is_err());
    }
}
