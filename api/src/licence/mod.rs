//! The licence core: keys and secrets, the signed token, and what the
//! extension's three requests do (activate, refresh, deactivate).
//!
//! The contract with the extension (bodies, codes, the token, the rules for
//! its dates) is `contract/licence-v1/`; this module follows it.

pub mod activate;
pub mod key;
pub mod refresh;
pub mod seats;
pub mod secret;
pub mod signer;
pub mod token;

use crate::store::licences::LicenceRow;
use token::{Entitlement, RevokeReason, Spans, Standing, Term};

/// What the licence logic reads from the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LicenceSettings {
    pub spans: Spans,
    /// An active installation not seen for this many days loses its seat.
    pub release_after_days: u32,
    /// New activations allowed in 30 days, per seat.
    pub activation_churn: u32,
}

impl Default for LicenceSettings {
    fn default() -> Self {
        Self {
            spans: Spans::default(),
            release_after_days: 60,
            activation_churn: 3,
        }
    }
}

/// `n` bytes from the operating system's random source.
pub fn random_bytes<const N: usize>() -> anyhow::Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes)
        .map_err(|err| anyhow::anyhow!("the system random source failed: {err}"))?;
    Ok(bytes)
}

/// The field rules of the request bodies (the contract's request schemas).
pub mod wire {
    fn base64url(text: &str) -> bool {
        text.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }

    /// A key as the extension sends it: 16–128 printable ASCII characters,
    /// no spaces.
    pub fn is_key_text(text: &str) -> bool {
        (16..=128).contains(&text.len()) && text.bytes().all(|b| (0x21..=0x7e).contains(&b))
    }

    /// An installation id: 22 base64url characters (16 bytes).
    pub fn is_inst(text: &str) -> bool {
        text.len() == 22 && base64url(text)
    }

    /// A licence id as the contract allows it: 8–64 characters of
    /// `[A-Za-z0-9_-]`.
    pub fn is_lid(text: &str) -> bool {
        (8..=64).contains(&text.len()) && base64url(text)
    }

    /// An installation secret: 32–128 base64url characters.
    pub fn is_secret(text: &str) -> bool {
        (32..=128).contains(&text.len()) && base64url(text)
    }

    /// The first four characters of an installation id: all a log line shows.
    pub fn short(inst: &str) -> &str {
        inst.get(..4).unwrap_or(inst)
    }
}

/// What a licence row means for the token (the backend spec's table of
/// inputs: `paid`, the end of what was paid, `term`, `renews`).
pub fn entitlement(licence: &LicenceRow) -> Entitlement {
    let standing = match (licence.status.as_str(), licence.revoke_reason.as_deref()) {
        ("revoked", Some("refund")) => Standing::Revoked(RevokeReason::Refund),
        ("revoked", _) => Standing::Revoked(RevokeReason::KeyBlocked),
        ("ended", _) => Standing::Ended,
        _ => Standing::Active,
    };
    let paid = licence.paid_until.map(time::OffsetDateTime::unix_timestamp);
    match licence.kind.as_str() {
        "subscription" => Entitlement {
            standing,
            paid,
            term: licence.term.as_deref().and_then(Term::parse),
            renews: !licence.cancel_at_period_end,
        },
        // A trial reads as a complimentary licence with an end: `paid` is its
        // end, and without a `term` there is no payment grace after it.
        "complimentary" | "trial" => Entitlement {
            standing,
            paid,
            term: None,
            renews: false,
        },
        // A one-time purchase has no end (`paid: null`).
        _ => Entitlement {
            standing,
            paid: None,
            term: None,
            renews: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::wire;

    #[test]
    fn wire_rules_follow_the_contract() {
        assert!(wire::is_key_text("DTD-ABCD-EFGH-JKMN-PQRS-TVWX"));
        assert!(!wire::is_key_text("DTD ABCD EFGH JKMN"), "spaces");
        assert!(!wire::is_key_text("short-key"), "fewer than 16");
        assert!(!wire::is_key_text(&"x".repeat(129)));
        assert!(
            !wire::is_key_text("DTD-ABCD-EFGH-JKMN-PQRS-TVWЖ"),
            "not ASCII"
        );
        assert!(wire::is_inst("q3V2mZ0bTEST0000000000"));
        assert!(!wire::is_inst("q3V2mZ0bTEST000000000="));
        assert!(!wire::is_inst("q3V2mZ0bTEST00000000000"));
        assert!(wire::is_lid("lic_01JD5QX7Y2Z9TEST"));
        assert!(!wire::is_lid("lic.1234"));
        assert!(wire::is_secret(&"A".repeat(43)));
        assert!(!wire::is_secret(&"A".repeat(31)));
        assert_eq!(wire::short("q3V2mZ0bTEST0000000000"), "q3V2");
    }
}
