//! The licence token: a compact JWS signed with ES256, its fields, and the
//! rules for its dates (`contract/licence-v1/`, the token schemas).
//!
//! What is signed is the ASCII of `<header>.<claims>`, each part base64url
//! without padding; the signature is 64 bytes `r‖s`. Times on the wire are
//! whole Unix seconds.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ALGORITHM: &str = "ES256";
pub const TOKEN_TYPE: &str = "devtab-licence";
/// A longer token is malformed.
pub const MAX_TOKEN_LEN: usize = 4096;
const DAY: i64 = 86_400;

/// The header: exactly these three fields, in this order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub alg: String,
    pub kid: String,
    pub typ: String,
}

impl Header {
    pub fn new(kid: &str) -> Self {
        Self {
            alg: ALGORITHM.to_owned(),
            kid: kid.to_owned(),
            typ: TOKEN_TYPE.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Active,
    Ended,
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RevokeReason {
    Refund,
    Deactivated,
    KeyBlocked,
    SeatLimit,
    UnknownInstallation,
}

impl RevokeReason {
    pub const ALL: [Self; 5] = [
        Self::Refund,
        Self::Deactivated,
        Self::KeyBlocked,
        Self::SeatLimit,
        Self::UnknownInstallation,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Refund => "refund",
            Self::Deactivated => "deactivated",
            Self::KeyBlocked => "key-blocked",
            Self::SeatLimit => "seat-limit",
            Self::UnknownInstallation => "unknown-installation",
        }
    }
}

/// The billing period of a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Term {
    Month,
    Year,
}

impl Term {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "month" => Some(Self::Month),
            "year" => Some(Self::Year),
            _ => None,
        }
    }
}

/// The token's fields, as on the wire. `reason` only with `revoked`; `term`
/// and `renews` never with `revoked`; `paid` is always there, maybe null.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub v: u8,
    pub lid: String,
    pub inst: String,
    pub plan: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<RevokeReason>,
    pub iat: i64,
    pub chk: i64,
    pub warn: i64,
    pub exp: i64,
    pub paid: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term: Option<Term>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renews: Option<bool>,
}

/// The spans the dates are built from, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spans {
    /// After how long the extension checks again (`chk`).
    pub check: i64,
    /// How long a token works without a check (`exp`).
    pub window: i64,
    /// How long before `exp` the extension warns (`warn`).
    pub warn_before: i64,
    /// How long a subscription keeps working after its paid period while the
    /// provider retries the payment.
    pub dunning: i64,
}

impl Spans {
    pub fn from_days(check: u32, window: u32, warn_before: u32, dunning: u32) -> Self {
        Self {
            check: i64::from(check) * DAY,
            window: i64::from(window) * DAY,
            warn_before: i64::from(warn_before) * DAY,
            dunning: i64::from(dunning) * DAY,
        }
    }
}

impl Default for Spans {
    /// 7, 30, 7 and 14 days.
    fn default() -> Self {
        Self::from_days(7, 30, 7, 14)
    }
}

/// Where a licence stands in the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Active,
    Ended,
    Revoked(RevokeReason),
}

/// What the token is issued from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entitlement {
    pub standing: Standing,
    /// The end of the paid period (a subscription), or of a complimentary
    /// licence; `None` for none.
    pub paid: Option<i64>,
    /// `Some` exactly for a subscription.
    pub term: Option<Term>,
    /// A subscription that renews at `paid` (not cancelled).
    pub renews: bool,
}

impl Entitlement {
    /// The end of what was paid: a subscription's `paid` plus the dunning
    /// grace, a complimentary licence's end, or `None`.
    pub fn end(&self, spans: &Spans) -> Option<i64> {
        self.paid.map(|paid| {
            paid + if self.term.is_some() {
                spans.dunning
            } else {
                0
            }
        })
    }

    /// Whether a token issued at `iat` says `active`.
    pub fn is_active_at(&self, iat: i64, spans: &Spans) -> bool {
        self.standing == Standing::Active && self.end(spans).is_none_or(|end| end > iat)
    }
}

/// What the server signs for this installation at `iat`:
///
/// - `revoked`: `chk = warn = exp = iat`, a reason, no `term`, no `renews`;
/// - `ended`, also for an active licence whose paid period and its grace are
///   over (`end ≤ iat`): `warn = exp = iat`, `chk = iat + check`, `term` of a
///   subscription, `renews: false`;
/// - `active` without an end: `chk = iat + check`, `exp = iat + window`,
///   `warn = exp − warn_before`, `paid: null`;
/// - `active` with `end > iat`: `exp = min(iat + window, end)`,
///   `chk = min(iat + check, exp)`, `warn = max(exp − warn_before, chk)`.
pub fn issue(lid: &str, inst: &str, iat: i64, licence: &Entitlement, spans: &Spans) -> Claims {
    let base = |status, reason, chk, warn, exp, paid, term, renews| Claims {
        v: 1,
        lid: lid.to_owned(),
        inst: inst.to_owned(),
        plan: "pro".to_owned(),
        status,
        reason,
        iat,
        chk,
        warn,
        exp,
        paid,
        term,
        renews,
    };
    if let Standing::Revoked(reason) = licence.standing {
        return base(
            Status::Revoked,
            Some(reason),
            iat,
            iat,
            iat,
            licence.paid,
            None,
            None,
        );
    }
    let end = licence.end(spans);
    if licence.standing == Standing::Ended || end.is_some_and(|end| end <= iat) {
        return base(
            Status::Ended,
            None,
            iat + spans.check,
            iat,
            iat,
            licence.paid,
            licence.term,
            Some(false),
        );
    }
    let Some(end) = end else {
        let exp = iat + spans.window;
        return base(
            Status::Active,
            None,
            iat + spans.check,
            exp - spans.warn_before,
            exp,
            None,
            None,
            None,
        );
    };
    let exp = (iat + spans.window).min(end);
    let chk = (iat + spans.check).min(exp);
    let warn = (exp - spans.warn_before).max(chk);
    let renews = if licence.term.is_some() {
        licence.renews
    } else {
        false
    };
    base(
        Status::Active,
        None,
        chk,
        warn,
        exp,
        licence.paid,
        licence.term,
        Some(renews),
    )
}

/// A token signed for every installation the server does not know: refresh
/// answers it with the `lid` and `inst` of the request.
pub fn unknown_installation(lid: &str, inst: &str, iat: i64, spans: &Spans) -> Claims {
    let licence = Entitlement {
        standing: Standing::Revoked(RevokeReason::UnknownInstallation),
        paid: None,
        term: None,
        renews: false,
    };
    issue(lid, inst, iat, &licence, spans)
}

fn encode_json<T: Serialize>(value: &T) -> String {
    // Serialising these plain structs cannot fail.
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap_or_default())
}

/// `<header>.<claims>`: the bytes the signature covers.
pub fn signing_input(header: &Header, claims: &Claims) -> String {
    format!("{}.{}", encode_json(header), encode_json(claims))
}

/// Why a token is refused, in the order the extension checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Malformed,
    Unsupported,
    UnknownKey,
    Signature,
    Claims,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::Unsupported => "unsupported",
            Self::UnknownKey => "unknown-key",
            Self::Signature => "signature",
            Self::Claims => "claims",
        }
    }
}

/// Strict base64url: the alphabet only, no padding, no length ≡ 1 (mod 4).
fn decode_part(part: &str) -> Option<Vec<u8>> {
    let strict = part
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !strict || part.len() % 4 == 1 {
        return None;
    }
    URL_SAFE_NO_PAD.decode(part).ok()
}

fn is_kid(kid: &str) -> bool {
    (1..=32).contains(&kid.len())
        && kid
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn header_is_valid(header: &Value) -> bool {
    let Some(fields) = header.as_object() else {
        return false;
    };
    fields.len() == 3
        && fields.get("alg").and_then(Value::as_str) == Some(ALGORITHM)
        && fields.get("typ").and_then(Value::as_str) == Some(TOKEN_TYPE)
        && fields
            .get("kid")
            .and_then(Value::as_str)
            .is_some_and(is_kid)
}

fn seconds(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64).filter(|n| *n >= 0)
}

/// The claims' rules (`claims` when broken, `unsupported` for another `v`).
fn check_claims(claims: &Value) -> Result<(), Refusal> {
    let fields = claims.as_object().ok_or(Refusal::Claims)?;
    let v = fields.get("v");
    // A number other than 1 is a version this check does not know.
    if v.is_some_and(|v| v.is_number() && v.as_u64() != Some(1)) {
        return Err(Refusal::Unsupported);
    }
    let text = |name: &str| fields.get(name).and_then(Value::as_str);
    let status = text("status");
    let reason = text("reason");
    let wire_ok = v.and_then(Value::as_i64) == Some(1)
        && text("lid").is_some_and(super::wire::is_lid)
        && text("inst").is_some_and(super::wire::is_inst)
        && text("plan") == Some("pro")
        && match status {
            Some("revoked") => {
                reason.is_some_and(|r| RevokeReason::ALL.iter().any(|k| k.as_str() == r))
            }
            Some("active" | "ended") => !fields.contains_key("reason"),
            _ => false,
        }
        && fields
            .get("paid")
            .is_some_and(|p| p.is_null() || seconds(Some(p)).is_some());
    let times = ["iat", "chk", "warn", "exp"].map(|name| seconds(fields.get(name)));
    let [Some(iat), Some(chk), Some(warn), Some(exp)] = times else {
        return Err(Refusal::Claims);
    };
    let ordered = if status == Some("active") {
        iat <= chk && chk <= warn && warn <= exp
    } else {
        warn == exp && exp == iat && iat <= chk
    };
    if wire_ok && ordered {
        Ok(())
    } else {
        Err(Refusal::Claims)
    }
}

/// Checks a token as the extension does: parts, header, signature size, key
/// id, signature, claims. Returns the header and the claims as JSON.
pub fn verify(token: &str, keys: &[(String, VerifyingKey)]) -> Result<(Value, Value), Refusal> {
    if token.len() > MAX_TOKEN_LEN {
        return Err(Refusal::Malformed);
    }
    let parts: Vec<&str> = token.split('.').collect();
    let [head, body, sig] = parts.as_slice() else {
        return Err(Refusal::Malformed);
    };
    let (Some(head_bytes), Some(body_bytes), Some(sig_bytes)) =
        (decode_part(head), decode_part(body), decode_part(sig))
    else {
        return Err(Refusal::Malformed);
    };
    let header: Value = serde_json::from_slice(&head_bytes).map_err(|_| Refusal::Malformed)?;
    let claims: Value = serde_json::from_slice(&body_bytes).map_err(|_| Refusal::Malformed)?;
    if !header.is_object() || !claims.is_object() {
        return Err(Refusal::Malformed);
    }
    if !header_is_valid(&header) {
        return Err(Refusal::Unsupported);
    }
    if sig_bytes.len() != 64 {
        return Err(Refusal::Malformed);
    }
    let kid = header["kid"].as_str().unwrap_or_default();
    let key = keys
        .iter()
        .find_map(|(k, key)| (k == kid).then_some(key))
        .ok_or(Refusal::UnknownKey)?;
    let signature = Signature::from_slice(&sig_bytes).map_err(|_| Refusal::Signature)?;
    key.verify(format!("{head}.{body}").as_bytes(), &signature)
        .map_err(|_| Refusal::Signature)?;
    check_claims(&claims)?;
    Ok((header, claims))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IAT: i64 = 1_795_392_000;
    const LID: &str = "lic_0000000000000000000000TEST";
    const INST: &str = "q3V2mZ0bTEST0000000000";

    fn days(d: f64) -> i64 {
        #[allow(clippy::cast_possible_truncation)]
        let seconds = (d * 86_400.0) as i64;
        seconds
    }

    fn subscription(paid_in_days: f64, term: Term, renews: bool) -> Entitlement {
        Entitlement {
            standing: Standing::Active,
            paid: Some(IAT + days(paid_in_days)),
            term: Some(term),
            renews,
        }
    }

    fn complimentary(end_in_days: Option<f64>) -> Entitlement {
        Entitlement {
            standing: Standing::Active,
            paid: end_in_days.map(|d| IAT + days(d)),
            term: None,
            renews: false,
        }
    }

    /// `(status, chk, warn, exp)` in days from `iat`.
    fn spans_of(claims: &Claims) -> (Status, f64, f64, f64) {
        #[allow(clippy::cast_precision_loss)]
        let d = |t: i64| (t - claims.iat) as f64 / 86_400.0;
        (claims.status, d(claims.chk), d(claims.warn), d(claims.exp))
    }

    fn issued(licence: &Entitlement) -> Claims {
        issue(LID, INST, IAT, licence, &Spans::default())
    }

    // The table of cases of the contract (§5.1 of the licence specification),
    // one test per row.

    #[test]
    fn case_01_subscription_paid_30_days_ahead() {
        let c = issued(&subscription(30.0, Term::Month, true));
        assert_eq!(spans_of(&c), (Status::Active, 7.0, 23.0, 30.0));
        assert_eq!((c.term, c.renews), (Some(Term::Month), Some(true)));
        assert_eq!(c.paid, Some(IAT + days(30.0)));
    }

    #[test]
    fn case_02_annual_subscription_paid_365_days() {
        let c = issued(&subscription(365.0, Term::Year, true));
        assert_eq!(spans_of(&c), (Status::Active, 7.0, 23.0, 30.0));
        assert_eq!(c.term, Some(Term::Year));
    }

    #[test]
    fn case_03_cancelled_paid_20_days() {
        let c = issued(&subscription(20.0, Term::Month, false));
        assert_eq!(spans_of(&c), (Status::Active, 7.0, 23.0, 30.0));
        assert_eq!(c.renews, Some(false));
    }

    #[test]
    fn case_04_paid_10_days_ahead() {
        let c = issued(&subscription(10.0, Term::Month, true));
        assert_eq!(spans_of(&c), (Status::Active, 7.0, 17.0, 24.0));
    }

    #[test]
    fn case_05_paid_until_today() {
        let c = issued(&subscription(0.0, Term::Month, true));
        assert_eq!(spans_of(&c), (Status::Active, 7.0, 7.0, 14.0));
    }

    #[test]
    fn case_06_payment_3_days_overdue() {
        let c = issued(&subscription(-3.0, Term::Month, true));
        assert_eq!(spans_of(&c), (Status::Active, 7.0, 7.0, 11.0));
    }

    #[test]
    fn case_07_payment_8_days_overdue_has_no_warning() {
        let c = issued(&subscription(-8.0, Term::Month, true));
        assert_eq!(spans_of(&c), (Status::Active, 6.0, 6.0, 6.0));
    }

    #[test]
    fn case_08_payment_13_and_a_half_days_overdue() {
        let c = issued(&subscription(-13.5, Term::Month, true));
        assert_eq!(spans_of(&c), (Status::Active, 0.5, 0.5, 0.5));
    }

    #[test]
    fn case_09_payment_exactly_14_days_overdue_is_ended() {
        let c = issued(&subscription(-14.0, Term::Month, true));
        assert_eq!(spans_of(&c), (Status::Ended, 7.0, 0.0, 0.0));
        assert_eq!(c.paid, Some(IAT - days(14.0)));
        assert_eq!((c.term, c.renews), (Some(Term::Month), Some(false)));
    }

    #[test]
    fn case_10_payment_20_days_overdue_is_ended() {
        let c = issued(&subscription(-20.0, Term::Month, true));
        assert_eq!(spans_of(&c), (Status::Ended, 7.0, 0.0, 0.0));
    }

    #[test]
    fn case_11_complimentary_without_an_end() {
        let c = issued(&complimentary(None));
        assert_eq!(spans_of(&c), (Status::Active, 7.0, 23.0, 30.0));
        assert_eq!((c.paid, c.term, c.renews), (None, None, None));
    }

    #[test]
    fn case_12_complimentary_until_three_days_from_now() {
        let c = issued(&complimentary(Some(3.0)));
        assert_eq!(spans_of(&c), (Status::Active, 3.0, 3.0, 3.0));
        assert_eq!((c.term, c.renews), (None, Some(false)));
    }

    #[test]
    fn case_13_complimentary_until_forty_days_from_now() {
        let c = issued(&complimentary(Some(40.0)));
        assert_eq!(spans_of(&c), (Status::Active, 7.0, 23.0, 30.0));
    }

    #[test]
    fn case_14_complimentary_until_yesterday_is_ended() {
        let c = issued(&complimentary(Some(-1.0)));
        assert_eq!(spans_of(&c), (Status::Ended, 7.0, 0.0, 0.0));
        assert_eq!((c.term, c.renews), (None, Some(false)));
    }

    #[test]
    fn case_15_subscription_that_ended() {
        let licence = Entitlement {
            standing: Standing::Ended,
            ..subscription(-40.0, Term::Year, false)
        };
        let c = issued(&licence);
        assert_eq!(spans_of(&c), (Status::Ended, 7.0, 0.0, 0.0));
        assert_eq!((c.term, c.renews), (Some(Term::Year), Some(false)));
    }

    #[test]
    fn case_16_revoked_for_each_reason() {
        for reason in RevokeReason::ALL {
            let licence = Entitlement {
                standing: Standing::Revoked(reason),
                ..subscription(10.0, Term::Month, true)
            };
            let c = issued(&licence);
            assert_eq!(spans_of(&c), (Status::Revoked, 0.0, 0.0, 0.0), "{reason:?}");
            assert_eq!(c.reason, Some(reason));
            assert_eq!((c.term, c.renews), (None, None), "never on revoked");
            assert_eq!(c.paid, Some(IAT + days(10.0)), "the paid date stays");
        }
    }

    #[test]
    fn an_active_token_always_has_iat_before_chk_and_ordered_dates() {
        let spans = Spans::default();
        for hours in -24 * 20..24 * 400 {
            for licence in [
                subscription(f64::from(hours) / 24.0, Term::Month, true),
                complimentary(Some(f64::from(hours) / 24.0)),
            ] {
                let c = issue(LID, INST, IAT, &licence, &spans);
                if c.status == Status::Active {
                    assert!(c.iat < c.chk && c.chk <= c.warn && c.warn <= c.exp, "{c:?}");
                    assert!(licence.is_active_at(IAT, &spans));
                } else {
                    assert_eq!(c.status, Status::Ended);
                    assert!(!licence.is_active_at(IAT, &spans));
                }
            }
        }
    }

    #[test]
    fn unknown_installation_is_revoked_without_a_paid_date() {
        let c = unknown_installation(LID, INST, IAT, &Spans::default());
        assert_eq!(c.status, Status::Revoked);
        assert_eq!(c.reason, Some(RevokeReason::UnknownInstallation));
        assert_eq!(c.paid, None);
    }

    #[test]
    fn the_claims_serialise_with_null_paid_and_without_absent_fields() {
        let json = serde_json::to_value(issued(&complimentary(None))).unwrap();
        assert!(json["paid"].is_null());
        assert!(json.get("reason").is_none());
        assert!(json.get("term").is_none());
        assert!(json.get("renews").is_none());
        let json = serde_json::to_value(issued(&Entitlement {
            standing: Standing::Revoked(RevokeReason::SeatLimit),
            ..complimentary(None)
        }))
        .unwrap();
        assert_eq!(json["reason"], "seat-limit");
    }

    #[test]
    fn the_header_is_exactly_alg_kid_typ() {
        let json = serde_json::to_string(&Header::new("test-0a1b2c3d")).unwrap();
        assert_eq!(
            json,
            r#"{"alg":"ES256","kid":"test-0a1b2c3d","typ":"devtab-licence"}"#
        );
    }

    #[test]
    fn strict_base64url_refuses_padding_and_other_alphabets() {
        assert!(decode_part("eyJh").is_some());
        assert!(decode_part("eyJh=").is_none());
        assert!(decode_part("ey+h").is_none());
        assert!(decode_part("ey/h").is_none());
        assert!(decode_part("ey h").is_none());
        assert!(decode_part("eyJhb").is_none(), "length 1 mod 4");
    }

    #[test]
    fn claims_rules() {
        let good = serde_json::to_value(issued(&subscription(30.0, Term::Month, true))).unwrap();
        assert_eq!(check_claims(&good), Ok(()));
        let mut extra = good.clone();
        extra["future"] = Value::from(1);
        assert_eq!(check_claims(&extra), Ok(()), "extra fields are allowed");
        let mut v2 = good.clone();
        v2["v"] = Value::from(2);
        assert_eq!(check_claims(&v2), Err(Refusal::Unsupported));
        for (field, value) in [
            ("plan", Value::from("free")),
            ("reason", Value::from("refund")),
            ("chk", Value::from(IAT - 1)),
            ("paid", Value::from("soon")),
            ("lid", Value::from("x")),
        ] {
            let mut broken = good.clone();
            broken[field] = value;
            assert_eq!(check_claims(&broken), Err(Refusal::Claims), "{field}");
        }
        let mut missing = good;
        missing.as_object_mut().unwrap().remove("paid");
        assert_eq!(check_claims(&missing), Err(Refusal::Claims), "paid");
    }
}
