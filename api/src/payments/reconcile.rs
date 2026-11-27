//! The rule that turns a subscription's state at its provider into the state
//! of its licence. Pure: no database, no network, the time given.
//!
//! - A licence is issued only for an active subscription whose current
//!   period is paid; an incomplete one waits for its payment.
//! - "Paid until" moves to the period's end only while the subscription is
//!   active and that period's invoice is paid. A provider starts the next
//!   period before it charges for it, and keeps moving it while it retries,
//!   so an unpaid period's end is never taken as paid.
//! - The first time a subscription is seen overdue marks the start of the
//!   grace (`past_due_since`); a paid period clears it.
//! - An ended or paused subscription ends the licence; the same subscription
//!   resumed and paid makes it active again.
//! - A revoked licence (refund, blocked key) stays revoked.

use time::OffsetDateTime;

use super::{SubscriptionSnapshot, SubscriptionStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenceStatus {
    Active,
    Ended,
    Revoked,
}

impl LicenceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Ended => "ended",
            Self::Revoked => "revoked",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "active" => Some(Self::Active),
            "ended" => Some(Self::Ended),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }
}

/// The part of a licence a subscription decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LicenceState {
    pub status: LicenceStatus,
    pub paid_until: Option<OffsetDateTime>,
    pub cancel_at_period_end: bool,
    pub past_due_since: Option<OffsetDateTime>,
    pub ended_at: Option<OffsetDateTime>,
}

/// What to do with the licence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// No licence yet, and the subscription is paid: issue one in this state.
    Issue(LicenceState),
    /// Write this state.
    Update(LicenceState),
    /// Nothing changes.
    Keep,
    /// No licence, and none is due yet (the first payment is pending, or the
    /// subscription ended before it was paid).
    Wait,
}

/// Decides the licence of `snapshot`; `before` is its licence, if any.
pub fn reconcile(
    before: Option<&LicenceState>,
    snapshot: &SubscriptionSnapshot,
    now: OffsetDateTime,
) -> Decision {
    let paid = snapshot.status == SubscriptionStatus::Active && snapshot.period_paid;
    let Some(before) = before else {
        return if paid {
            Decision::Issue(LicenceState {
                status: LicenceStatus::Active,
                paid_until: Some(snapshot.current_period_end),
                cancel_at_period_end: snapshot.cancel_at_period_end,
                past_due_since: None,
                ended_at: None,
            })
        } else {
            Decision::Wait
        };
    };
    if before.status == LicenceStatus::Revoked {
        return Decision::Keep;
    }
    let after = match snapshot.status {
        SubscriptionStatus::Active if snapshot.period_paid => LicenceState {
            status: LicenceStatus::Active,
            paid_until: Some(snapshot.current_period_end),
            cancel_at_period_end: snapshot.cancel_at_period_end,
            past_due_since: None,
            ended_at: None,
        },
        // A renewal being charged: the paid period stays where it was.
        SubscriptionStatus::Active | SubscriptionStatus::Incomplete => LicenceState {
            cancel_at_period_end: snapshot.cancel_at_period_end,
            ..*before
        },
        SubscriptionStatus::PastDue => LicenceState {
            status: LicenceStatus::Active,
            cancel_at_period_end: snapshot.cancel_at_period_end,
            past_due_since: before.past_due_since.or(Some(now)),
            ended_at: None,
            ..*before
        },
        SubscriptionStatus::Ended | SubscriptionStatus::Paused => LicenceState {
            status: LicenceStatus::Ended,
            ended_at: before.ended_at.or(Some(now)),
            ..*before
        },
    };
    if after == *before {
        Decision::Keep
    } else {
        Decision::Update(after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_796_083_200 + day * 86_400).unwrap()
    }

    fn snapshot(status: SubscriptionStatus, period_end: i64, paid: bool) -> SubscriptionSnapshot {
        SubscriptionSnapshot {
            id: "sub_1".to_owned(),
            status,
            price_id: "price_month".to_owned(),
            current_period_end: at(period_end),
            cancel_at_period_end: false,
            customer: "cus_1".to_owned(),
            period_paid: paid,
            checkout: None,
        }
    }

    fn active(paid_until: i64) -> LicenceState {
        LicenceState {
            status: LicenceStatus::Active,
            paid_until: Some(at(paid_until)),
            cancel_at_period_end: false,
            past_due_since: None,
            ended_at: None,
        }
    }

    use SubscriptionStatus::{Active, Ended, Incomplete, PastDue, Paused};

    #[test]
    fn a_paid_subscription_issues_a_licence_and_an_unpaid_one_waits() {
        assert_eq!(
            reconcile(None, &snapshot(Active, 30, true), at(0)),
            Decision::Issue(active(30))
        );
        for unpaid in [
            snapshot(Active, 30, false),
            snapshot(Incomplete, 30, false),
            snapshot(PastDue, 30, false),
            snapshot(Ended, 30, true),
        ] {
            assert_eq!(
                reconcile(None, &unpaid, at(0)),
                Decision::Wait,
                "{unpaid:?}"
            );
        }
    }

    #[test]
    fn a_paid_renewal_moves_paid_until_and_an_unpaid_one_does_not() {
        let before = active(30);
        assert_eq!(
            reconcile(Some(&before), &snapshot(Active, 60, true), at(30)),
            Decision::Update(active(60))
        );
        // The provider started the new period but has not charged for it.
        assert_eq!(
            reconcile(Some(&before), &snapshot(Active, 60, false), at(30)),
            Decision::Keep
        );
        // The same state again changes nothing.
        assert_eq!(
            reconcile(Some(&active(60)), &snapshot(Active, 60, true), at(31)),
            Decision::Keep
        );
    }

    #[test]
    fn overdue_freezes_paid_until_and_marks_the_first_time_seen() {
        let before = active(30);
        let Decision::Update(overdue) =
            reconcile(Some(&before), &snapshot(PastDue, 60, false), at(31))
        else {
            panic!("an update");
        };
        assert_eq!(overdue.status, LicenceStatus::Active);
        assert_eq!(
            overdue.paid_until,
            Some(at(30)),
            "the unpaid period is not paid"
        );
        assert_eq!(overdue.past_due_since, Some(at(31)));
        // Seen overdue again later: the mark stays at the first time.
        assert_eq!(
            reconcile(Some(&overdue), &snapshot(PastDue, 60, false), at(35)),
            Decision::Keep
        );
        // Paid at last: the new period, and the mark cleared.
        assert_eq!(
            reconcile(Some(&overdue), &snapshot(Active, 60, true), at(36)),
            Decision::Update(active(60))
        );
    }

    #[test]
    fn cancelling_at_the_period_s_end_keeps_the_licence_active() {
        let mut cancelling = snapshot(Active, 30, true);
        cancelling.cancel_at_period_end = true;
        let Decision::Update(after) = reconcile(Some(&active(30)), &cancelling, at(10)) else {
            panic!("an update");
        };
        assert_eq!(after.status, LicenceStatus::Active);
        assert!(after.cancel_at_period_end);
        assert_eq!(after.paid_until, Some(at(30)));
    }

    #[test]
    fn an_ended_or_paused_subscription_ends_the_licence_once() {
        for status in [Ended, Paused] {
            let Decision::Update(ended) =
                reconcile(Some(&active(30)), &snapshot(status, 30, true), at(30))
            else {
                panic!("an update");
            };
            assert_eq!(ended.status, LicenceStatus::Ended);
            assert_eq!(ended.ended_at, Some(at(30)));
            assert_eq!(ended.paid_until, Some(at(30)));
            assert_eq!(
                reconcile(Some(&ended), &snapshot(status, 30, true), at(40)),
                Decision::Keep,
                "the end is recorded once"
            );
        }
    }

    #[test]
    fn the_same_subscription_resumed_and_paid_is_active_again() {
        let ended = LicenceState {
            status: LicenceStatus::Ended,
            ended_at: Some(at(30)),
            ..active(30)
        };
        assert_eq!(
            reconcile(Some(&ended), &snapshot(Active, 90, true), at(60)),
            Decision::Update(active(90))
        );
    }

    #[test]
    fn a_revoked_licence_stays_revoked() {
        let revoked = LicenceState {
            status: LicenceStatus::Revoked,
            ..active(30)
        };
        for state in [
            snapshot(Active, 60, true),
            snapshot(PastDue, 60, false),
            snapshot(Ended, 60, false),
        ] {
            assert_eq!(reconcile(Some(&revoked), &state, at(40)), Decision::Keep);
        }
    }
}
