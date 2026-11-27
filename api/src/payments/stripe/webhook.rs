//! The check of a Stripe webhook before anything reads it, as Stripe's
//! "verify manually" describes: the `Stripe-Signature` header carries a
//! time `t` and one or more `v1` signatures, each an HMAC-SHA256 with the
//! destination's secret over `t`, a dot and the raw body. The body is parsed
//! only after a signature matched, the time is within the tolerance and the
//! event's mode is the server's.

use hmac::{Hmac, KeyInit as _, Mac as _};
use serde_json::Value;
use sha2::Sha256;
use subtle::ConstantTimeEq as _;
use time::OffsetDateTime;

use super::StripeMode;
use super::keys::WebhookSecret;

/// An event whose signature, time and mode were checked.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedEvent {
    /// `evt_…`: a repeated delivery has the same id.
    pub id: String,
    /// `type`, such as `checkout.session.completed`.
    pub kind: String,
    /// Seconds since 1970 when Stripe created the event; not an order.
    pub created: i64,
    /// The version the destination renders events in.
    pub api_version: Option<String>,
    /// `data.object`: the object as the event saw it.
    pub object: Value,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum WebhookError {
    #[error("the Stripe-Signature header is malformed: {0}")]
    Malformed(&'static str),
    #[error("the signature is {0} seconds away from now, over the tolerance")]
    Stale(i64),
    #[error("no signature matches the destination's secret")]
    Mismatch,
    #[error("the event is from Stripe's {got} mode, the server is in {want}")]
    WrongMode { got: StripeMode, want: StripeMode },
    #[error("the body is not a Stripe event: {0}")]
    Body(String),
}

/// Checks a delivery: `header` is `Stripe-Signature`, `body` the raw bytes as
/// they came, `now` the server's clock.
pub fn verify(
    header: &str,
    body: &[u8],
    secret: &WebhookSecret,
    now: OffsetDateTime,
    tolerance_s: u32,
    mode: StripeMode,
) -> Result<VerifiedEvent, WebhookError> {
    let (timestamp, signatures) = parse_header(header)?;
    let age = now.unix_timestamp() - timestamp;
    if age.unsigned_abs() > u64::from(tolerance_s) {
        return Err(WebhookError::Stale(age));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.expose())
        .map_err(|_| WebhookError::Malformed("the secret cannot key an HMAC"))?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    // Every signature is compared; any one matching is enough (Stripe signs
    // with the old and the new secret while a secret is being rolled).
    let matched = signatures.iter().fold(0u8, |any, sig| {
        any | sig.ct_eq(expected.as_slice()).unwrap_u8()
    });
    if matched == 0 {
        return Err(WebhookError::Mismatch);
    }
    read_event(body, mode)
}

/// `t=<seconds>` once, `v1=<64 hex digits>` at least once; other elements
/// (`v0`, test-mode or debugging ones) are skipped, in any order.
fn parse_header(header: &str) -> Result<(i64, Vec<[u8; 32]>), WebhookError> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for element in header.split(',') {
        let Some((key, value)) = element.trim().split_once('=') else {
            continue;
        };
        match key {
            "t" => {
                if timestamp.is_some() {
                    return Err(WebhookError::Malformed("two timestamps"));
                }
                timestamp = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| WebhookError::Malformed("the timestamp is not a number"))?,
                );
            }
            "v1" => {
                // A v1 that is not 64 hex digits cannot match; skip it.
                if let Some(signature) = hex32(value) {
                    signatures.push(signature);
                }
            }
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or(WebhookError::Malformed("no timestamp"))?;
    if signatures.is_empty() {
        return Err(WebhookError::Malformed("no v1 signature"));
    }
    Ok((timestamp, signatures))
}

fn hex32(text: &str) -> Option<[u8; 32]> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (i, pair) in bytes.chunks_exact(2).enumerate() {
        out[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(out)
}

fn read_event(body: &[u8], mode: StripeMode) -> Result<VerifiedEvent, WebhookError> {
    let event: Value =
        serde_json::from_slice(body).map_err(|err| WebhookError::Body(err.to_string()))?;
    let field = |name: &'static str| {
        event
            .get(name)
            .ok_or_else(|| WebhookError::Body(format!("no `{name}`")))
    };
    let text = |name: &'static str| {
        field(name)?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| WebhookError::Body(format!("`{name}` is not text")))
    };
    let livemode = field("livemode")?
        .as_bool()
        .ok_or_else(|| WebhookError::Body("`livemode` is not true or false".to_owned()))?;
    if livemode != mode.is_live() {
        let got = if livemode {
            StripeMode::Live
        } else {
            StripeMode::Sandbox
        };
        return Err(WebhookError::WrongMode { got, want: mode });
    }
    Ok(VerifiedEvent {
        id: text("id")?,
        kind: text("type")?,
        created: field("created")?
            .as_i64()
            .ok_or_else(|| WebhookError::Body("`created` is not a number".to_owned()))?,
        api_version: event
            .get("api_version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        object: event
            .get("data")
            .and_then(|data| data.get("object"))
            .cloned()
            .ok_or_else(|| WebhookError::Body("no `data.object`".to_owned()))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"id":"evt_1","object":"event","type":"checkout.session.completed","created":1796104790,"api_version":"2026-11-18.endive","livemode":false,"data":{"object":{"id":"cs_test_1"}}}"#;
    /// HMAC-SHA256 of `1796104800.` + BODY with `whsec_test_secret`, computed
    /// outside this code (Python's `hmac`).
    const SIGNATURE: &str = "8342883ef6be065d7862118dbd5bd3d927c77b629e9cf5b097968d6ef5ee4893";
    const T: i64 = 1_796_104_800;

    fn secret() -> WebhookSecret {
        WebhookSecret::parse("whsec_test_secret").unwrap()
    }

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).unwrap()
    }

    fn check(header: &str, body: &str, now: i64) -> Result<VerifiedEvent, WebhookError> {
        verify(
            header,
            body.as_bytes(),
            &secret(),
            at(now),
            300,
            StripeMode::Sandbox,
        )
    }

    #[test]
    fn a_signed_event_is_read() {
        let event = check(&format!("t={T},v1={SIGNATURE}"), BODY, T + 10).unwrap();
        assert_eq!(event.id, "evt_1");
        assert_eq!(event.kind, "checkout.session.completed");
        assert_eq!(event.created, 1_796_104_790);
        assert_eq!(event.api_version.as_deref(), Some("2026-11-18.endive"));
        assert_eq!(event.object["id"], "cs_test_1");
    }

    #[test]
    fn any_matching_v1_is_enough_and_other_elements_are_skipped() {
        let other = "0".repeat(64);
        let header = format!(" v0={other}, t={T} ,v1={other},v1=short,v1={SIGNATURE},x=y");
        assert!(check(&header, BODY, T).is_ok());
        let upper = format!("t={T},v1={}", SIGNATURE.to_ascii_uppercase());
        assert!(check(&upper, BODY, T).is_ok());
    }

    #[test]
    fn a_changed_body_or_another_secret_does_not_match() {
        let header = format!("t={T},v1={SIGNATURE}");
        let changed = BODY.replace("cs_test_1", "cs_test_2");
        assert_eq!(check(&header, &changed, T), Err(WebhookError::Mismatch));
        let other = WebhookSecret::parse("whsec_other").unwrap();
        let result = verify(
            &header,
            BODY.as_bytes(),
            &other,
            at(T),
            300,
            StripeMode::Sandbox,
        );
        assert_eq!(result, Err(WebhookError::Mismatch));
        let moved = format!("t={},v1={SIGNATURE}", T + 1);
        assert_eq!(check(&moved, BODY, T), Err(WebhookError::Mismatch));
    }

    #[test]
    fn the_time_must_be_within_the_tolerance_both_ways() {
        let header = format!("t={T},v1={SIGNATURE}");
        assert!(check(&header, BODY, T + 300).is_ok());
        assert_eq!(check(&header, BODY, T + 301), Err(WebhookError::Stale(301)));
        assert_eq!(
            check(&header, BODY, T - 301),
            Err(WebhookError::Stale(-301))
        );
    }

    #[test]
    fn a_malformed_header_is_refused() {
        for header in [
            String::new(),
            format!("v1={SIGNATURE}"),
            format!("t={T}"),
            format!("t=soon,v1={SIGNATURE}"),
            format!("t={T},t={T},v1={SIGNATURE}"),
            format!("t={T},v1=xyz"),
        ] {
            assert!(
                matches!(check(&header, BODY, T), Err(WebhookError::Malformed(_))),
                "{header}"
            );
        }
    }

    #[test]
    fn an_event_of_the_other_mode_is_refused() {
        let result = verify(
            &format!("t={T},v1={SIGNATURE}"),
            BODY.as_bytes(),
            &secret(),
            at(T),
            300,
            StripeMode::Live,
        );
        assert_eq!(
            result,
            Err(WebhookError::WrongMode {
                got: StripeMode::Sandbox,
                want: StripeMode::Live
            })
        );
    }
}
