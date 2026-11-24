//! The contract with the extension, from the server's side
//! (`contract/licence-v1/`):
//!
//! - `server-vectors.json`: tokens this server signs, one per row of the
//!   issuing rules' table of cases and per revoke reason, plus broken ones,
//!   each with what the extension's `verifyToken` and `evaluate` must say.
//!   They are signed by a test key made for one run, whose public half is in
//!   the file and whose private half was never stored. The test checks that
//!   every token verifies with that public key, that its claims are what the
//!   code issues for its case, and the stated outcome and state;
//!   `DEVTAB_WRITE_VECTORS=1` makes a new key and writes the file again.
//!   `contract/licence-v1/verify.test.ts` runs the same file through the copy
//!   of the extension's own check.
//! - `extension-vectors.json` (copied by `script/contract-sync`): the
//!   extension's tokens through this server's check.
//! - `errors.json` against the API's error codes, the schemas against
//!   samples, `production-keys.json` against the published keys.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod support;

use axum::http::StatusCode;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use devtab_api::http::error::AppError;
use devtab_api::licence::signer::{self, Jwk, KeyList, Signer};
use devtab_api::licence::token::{self, Entitlement, RevokeReason, Spans, Standing, Term};
use serde_json::{Map, Value, json};
use support::schema::{assert_valid, load, problems};

const IAT: i64 = 1_795_392_000;
const LID: &str = "lic_0000000000000000000000TEST";
const INST: &str = "q3V2mZ0bTEST0000000000";
const DAY: i64 = 86_400;

/// A licence as the vectors describe it (the shape the extension's
/// reference `issueClaims` takes): `status`, `reason`, `paid`, and `term`
/// and `renews` for a subscription.
fn licence_json(rights: &Entitlement) -> Value {
    let mut object = Map::new();
    let (status, reason) = match rights.standing {
        Standing::Active => ("active", None),
        Standing::Ended => ("ended", None),
        Standing::Revoked(reason) => ("revoked", Some(reason.as_str())),
    };
    object.insert("status".into(), status.into());
    if let Some(reason) = reason {
        object.insert("reason".into(), reason.into());
    }
    object.insert("paid".into(), rights.paid.map_or(Value::Null, Value::from));
    if let Some(term) = rights.term {
        object.insert("term".into(), serde_json::to_value(term).unwrap());
        object.insert("renews".into(), rights.renews.into());
    }
    Value::Object(object)
}

fn subscription(paid_in_seconds: i64, term: Term, renews: bool) -> Entitlement {
    Entitlement {
        standing: Standing::Active,
        paid: Some(IAT + paid_in_seconds),
        term: Some(term),
        renews,
    }
}

fn complimentary(end_in_seconds: Option<i64>) -> Entitlement {
    Entitlement {
        standing: Standing::Active,
        paid: end_in_seconds.map(|s| IAT + s),
        term: None,
        renews: false,
    }
}

/// The table of cases of the issuing rules, in its order.
fn cases() -> Vec<(String, Entitlement)> {
    let mut cases = vec![
        (
            "case-01 subscription paid 30 days ahead",
            subscription(30 * DAY, Term::Month, true),
        ),
        (
            "case-02 annual subscription paid 365 days ahead",
            subscription(365 * DAY, Term::Year, true),
        ),
        (
            "case-03 cancelled, paid 20 days ahead",
            subscription(20 * DAY, Term::Month, false),
        ),
        (
            "case-04 paid 10 days ahead",
            subscription(10 * DAY, Term::Month, true),
        ),
        (
            "case-05 paid until today",
            subscription(0, Term::Month, true),
        ),
        (
            "case-06 payment 3 days overdue",
            subscription(-3 * DAY, Term::Month, true),
        ),
        (
            "case-07 payment 8 days overdue",
            subscription(-8 * DAY, Term::Month, true),
        ),
        (
            "case-08 payment 13.5 days overdue",
            subscription(-27 * DAY / 2, Term::Month, true),
        ),
        (
            "case-09 payment exactly 14 days overdue",
            subscription(-14 * DAY, Term::Month, true),
        ),
        (
            "case-10 payment 20 days overdue",
            subscription(-20 * DAY, Term::Month, true),
        ),
        ("case-11 complimentary without an end", complimentary(None)),
        (
            "case-12 complimentary until 3 days ahead",
            complimentary(Some(3 * DAY)),
        ),
        (
            "case-13 complimentary until 40 days ahead",
            complimentary(Some(40 * DAY)),
        ),
        (
            "case-14 complimentary until yesterday",
            complimentary(Some(-DAY)),
        ),
        (
            "case-15 subscription ended",
            Entitlement {
                standing: Standing::Ended,
                ..subscription(-40 * DAY, Term::Year, false)
            },
        ),
    ]
    .into_iter()
    .map(|(name, rights)| (name.to_owned(), rights))
    .collect::<Vec<_>>();
    for reason in RevokeReason::ALL {
        let paid = if reason == RevokeReason::Refund {
            Some(IAT + 10 * DAY)
        } else {
            None
        };
        cases.push((
            format!("case-16 revoked: {}", reason.as_str()),
            Entitlement {
                standing: Standing::Revoked(reason),
                paid,
                term: None,
                renews: false,
            },
        ));
    }
    cases
}

/// When a case's state is read: a day after `iat` for two cases, eight days
/// for the others, so that the states cover active (due or not), grace,
/// expired, ended and revoked.
fn now_ms(case: &str) -> i64 {
    let days = if case.starts_with("case-01") || case.starts_with("case-11") {
        1
    } else {
        8
    };
    (IAT + days * DAY) * 1000
}

/// What the extension's `evaluate` gives for a verified token at `now` with
/// the clock mark `seen` (the table of states, pro-entitlement §4.1), in the
/// shape of its result without the licence facts.
fn state_of(claims: &Value, now: i64, seen: i64) -> Value {
    let ms = |name: &str| claims[name].as_i64().unwrap() * 1000;
    match claims["status"].as_str().unwrap() {
        "ended" => json!({ "kind": "ended" }),
        "revoked" => json!({ "kind": "revoked", "reason": claims["reason"] }),
        _ => {
            let t = now.max(seen).max(ms("iat"));
            if t >= ms("exp") {
                json!({ "kind": "expired" })
            } else if t >= ms("warn") {
                json!({ "kind": "grace" })
            } else {
                json!({ "kind": "active", "due": t >= ms("chk") })
            }
        }
    }
}

/// The broken tokens: a changed signature, a key id nobody knows, `alg:
/// none`, a DER-sized signature, a payload that parses but is not an object,
/// and a payload that is not UTF-8; each with the extension's reason.
fn broken(signer: &Signer, good: &str) -> Vec<(&'static str, String, &'static str)> {
    let parts: Vec<&str> = good.split('.').collect();
    let mut signature = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
    let tail = signature.len() - 1;
    signature[tail] ^= 0x01;
    let encode = |bytes: &[u8]| URL_SAFE_NO_PAD.encode(bytes);
    let header = |value: Value| encode(value.to_string().as_bytes());
    let with_payload =
        |payload: &[u8]| signer.sign_input(&format!("{}.{}", parts[0], encode(payload)));
    vec![
        (
            "broken: one bit of the signature flipped",
            format!("{}.{}.{}", parts[0], parts[1], encode(&signature)),
            "signature",
        ),
        (
            "broken: a key id no build knows",
            format!(
                "{}.{}.{}",
                header(json!({ "alg": "ES256", "kid": "test-00000000", "typ": "devtab-licence" })),
                parts[1],
                parts[2]
            ),
            "unknown-key",
        ),
        (
            "broken: alg none without a signature",
            format!(
                "{}.{}.",
                header(json!({ "alg": "none", "kid": signer.kid(), "typ": "devtab-licence" })),
                parts[1]
            ),
            "unsupported",
        ),
        (
            "broken: a 70-byte signature",
            format!("{}.{}.{}", parts[0], parts[1], encode(&[7_u8; 70])),
            "malformed",
        ),
        (
            "broken: a signed payload that is JSON but not an object",
            with_payload(b"[1]"),
            "malformed",
        ),
        (
            "broken: a signed payload that is not UTF-8",
            with_payload(&[0xff, 0xfe, 0x7b]),
            "malformed",
        ),
    ]
}

/// The vectors file, signed by `signer`.
fn vectors(signer: &Signer) -> Value {
    let spans = Spans::default();
    let mut list = Vec::new();
    let mut first = None;
    for (name, rights) in cases() {
        let issued = token::issue(LID, INST, IAT, &rights, &spans);
        let claims = serde_json::to_value(&issued).unwrap();
        let token = signer.sign(&issued);
        first.get_or_insert_with(|| token.clone());
        let now = now_ms(&name);
        list.push(json!({
            "name": name,
            "licence": licence_json(&rights),
            "token": token,
            "inst": INST,
            "now": now,
            "seen": 0,
            "verify": "ok",
            "state": state_of(&claims, now, 0),
        }));
    }
    for (name, token, reason) in broken(signer, &first.unwrap()) {
        list.push(json!({
            "name": name,
            "token": token,
            "inst": INST,
            "now": (IAT + DAY) * 1000,
            "seen": 0,
            "verify": reason,
            "state": { "kind": "invalid", "reason": reason },
        }));
    }
    json!({
        "note": "DEVTAB_LICENCE_TEST. Test vectors of the licence token from the licence server: a public key made for one run of the backend's contract test (its private key was never stored) and tokens it signed, one per case of the issuing rules and per revoke reason, plus broken ones. Not a key or a token of the product. `verify` is the outcome of verifyToken against `keys` (\"ok\" or the reason); `state` is what evaluate gives for the record { inst, token } at `now` with the clock mark `seen` (ms). Write a new file with DEVTAB_WRITE_VECTORS=1 cargo test -p devtab-api --test contract server_vectors.",
        "iat": IAT,
        "keys": [{ "kid": signer.kid(), "jwk": signer.jwk() }],
        "vectors": list,
    })
}

fn vectors_path() -> std::path::PathBuf {
    support::repo("contract/licence-v1/server-vectors.json")
}

/// `DEVTAB_WRITE_VECTORS=1`: a key made now, the file written, the key gone.
fn write_vectors_when_asked() {
    if std::env::var("DEVTAB_WRITE_VECTORS").is_ok_and(|v| v == "1") {
        let (pem, entry) = signer::generate(None, true, time::OffsetDateTime::now_utc()).unwrap();
        let signer = Signer::from_pkcs8_pem(&entry.kid, &pem).unwrap();
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(&vectors(&signer)).unwrap()
        );
        std::fs::write(vectors_path(), text).unwrap();
    }
}

fn key_entries(file: &Value) -> Vec<(String, p256::ecdsa::VerifyingKey)> {
    file["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let jwk: Jwk = serde_json::from_value(entry["jwk"].clone()).unwrap();
            (
                entry["kid"].as_str().unwrap().to_owned(),
                jwk.verifying_key().unwrap(),
            )
        })
        .collect()
}

/// `expected` holds a part of `actual` (the extension's `toMatchObject`).
fn holds(actual: &Value, expected: &Value) -> bool {
    expected
        .as_object()
        .unwrap()
        .iter()
        .all(|(name, value)| actual.get(name) == Some(value))
}

/// The committed vectors: each token verifies with the file's public key (or
/// is refused for its stated reason), its claims are what the code issues
/// for its case now, and its stated state follows from them.
#[test]
fn server_vectors_hold() {
    write_vectors_when_asked();
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(vectors_path()).unwrap()).unwrap();
    assert_eq!(file["iat"], IAT);
    let keys = key_entries(&file);
    assert_eq!(keys.len(), 1);
    assert!(keys[0].0.starts_with("test-"), "a test key id");
    assert!(
        KeyList::published().unwrap().find(&keys[0].0).is_none(),
        "never published"
    );

    let spans = Spans::default();
    let cases = cases();
    let vectors = file["vectors"].as_array().unwrap();
    assert_eq!(
        vectors.len(),
        cases.len() + 6,
        "20 cases and 6 broken tokens"
    );
    let mut statuses = std::collections::BTreeSet::new();
    for (vector, (name, rights)) in vectors.iter().zip(&cases) {
        assert_eq!(vector["name"], name.as_str());
        assert_eq!(vector["licence"], licence_json(rights), "{name}");
        let (header, claims) = token::verify(vector["token"].as_str().unwrap(), &keys)
            .unwrap_or_else(|refusal| panic!("{name}: {}", refusal.as_str()));
        assert_valid("token.header.schema.json", &header);
        assert_valid("token.claims.schema.json", &claims);
        let issued = serde_json::to_value(token::issue(LID, INST, IAT, rights, &spans)).unwrap();
        assert_eq!(claims, issued, "{name}: what the code issues now");
        assert_eq!(vector["verify"], "ok");
        let now = vector["now"].as_i64().unwrap();
        assert_eq!(vector["state"], state_of(&claims, now, 0), "{name}");
        statuses.insert(format!("{}:{}", claims["status"], claims["reason"]));
    }
    // active, ended, and revoked with each of the five reasons.
    assert_eq!(statuses.len(), 7, "{statuses:?}");
    for vector in &vectors[cases.len()..] {
        let name = vector["name"].as_str().unwrap();
        let expected = vector["verify"].as_str().unwrap();
        let refusal = token::verify(vector["token"].as_str().unwrap(), &keys).unwrap_err();
        assert_eq!(refusal.as_str(), expected, "{name}");
        assert_eq!(
            vector["state"],
            json!({ "kind": "invalid", "reason": expected })
        );
    }
}

/// The extension's own vectors (copied by `script/contract-sync`): every
/// token gets the stated outcome from this server's check, and every
/// verified one the stated state.
#[test]
fn extension_vectors_pass_the_rust_check() {
    let sources =
        std::fs::read_to_string(support::repo("contract/licence-v1/SOURCES.sha256")).unwrap();
    assert!(
        sources.contains("  extension-vectors.json"),
        "copied with a checksum"
    );
    let text = std::fs::read_to_string(support::repo("contract/licence-v1/extension-vectors.json"))
        .unwrap();
    let file: Value = serde_json::from_str(&text).unwrap();
    let keys = key_entries(&file);
    let vectors = file["vectors"].as_array().unwrap();
    assert!(vectors.len() >= 8);
    for vector in vectors {
        let name = vector["name"].as_str().unwrap();
        match token::verify(vector["token"].as_str().unwrap(), &keys) {
            Ok((_, claims)) => {
                assert_eq!(vector["verify"], "ok", "{name}");
                assert_eq!(claims["inst"], vector["inst"], "{name}");
                let state = state_of(
                    &claims,
                    vector["now"].as_i64().unwrap(),
                    vector["seen"].as_i64().unwrap(),
                );
                assert!(
                    holds(&state, &vector["state"]),
                    "{name}: {state} vs {}",
                    vector["state"]
                );
            }
            Err(refusal) => assert_eq!(vector["verify"], refusal.as_str(), "{name}"),
        }
    }
}

#[test]
fn the_error_table_is_the_apis() {
    let errors = load("errors.json");
    let api = [
        AppError::BadRequest(String::new()),
        AppError::KeyBlocked,
        AppError::LicenceInactive,
        AppError::KeyUnknown,
        AppError::SeatLimit,
        AppError::PayloadTooLarge,
        AppError::RateLimited { retry_after_s: 1 },
        AppError::Unavailable,
    ];
    let codes = errors["codes"].as_object().unwrap();
    assert_eq!(codes.len(), api.len());
    for error in &api {
        assert_eq!(
            codes[error.code()],
            error.status().as_u16(),
            "{}",
            error.code()
        );
    }
    assert_eq!(
        AppError::UnavailableFor { retry_after_s: 1 }.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    for (endpoint, list) in errors["endpoints"].as_object().unwrap() {
        for code in list.as_array().unwrap() {
            assert!(
                codes.contains_key(code.as_str().unwrap()),
                "{endpoint}: {code}"
            );
        }
    }
}

#[test]
fn the_schemas_accept_good_bodies_and_refuse_broken_ones() {
    let inst = INST;
    let secret = "S".repeat(43);
    let good = [
        (
            "activate.request.schema.json",
            json!({ "key": "DTD-ABCD-EFGH-JKMN-PQRS-TVWX", "inst": inst }),
        ),
        (
            "activate.response.schema.json",
            json!({ "token": "a.b.c", "secret": secret }),
        ),
        (
            "refresh.request.schema.json",
            json!({ "lid": LID, "inst": inst, "secret": secret }),
        ),
        (
            "deactivate.request.schema.json",
            json!({ "lid": LID, "inst": inst, "secret": secret }),
        ),
        ("refresh.response.schema.json", json!({ "token": "a.b.c" })),
        (
            "error.schema.json",
            json!({ "error": "seat-limit", "message": "Full." }),
        ),
    ];
    for (name, body) in &good {
        assert_valid(name, body);
    }
    let broken = [
        (
            "activate.request.schema.json",
            json!({ "key": "DTD-ABCD-EFGH-JKMN-PQRS-TVWX" }),
        ),
        (
            "activate.request.schema.json",
            json!({ "key": "DTD ABCD EFGH JKMN PQRS", "inst": inst }),
        ),
        (
            "activate.request.schema.json",
            json!({ "key": "DTD-ABCD-EFGH-JKMN", "inst": "short", "x": 1 }),
        ),
        (
            "activate.response.schema.json",
            json!({ "token": "a.b.c", "secret": "short" }),
        ),
        (
            "refresh.request.schema.json",
            json!({ "lid": "lic", "inst": inst, "secret": secret }),
        ),
        (
            "error.schema.json",
            json!({ "error": "Seat Limit", "message": "x" }),
        ),
        ("error.schema.json", json!({ "error": "seat-limit" })),
        (
            "token.header.schema.json",
            json!({ "alg": "none", "kid": "p1", "typ": "devtab-licence" }),
        ),
        (
            "token.header.schema.json",
            json!({ "alg": "ES256", "kid": "p1", "typ": "devtab-licence", "x5u": "x" }),
        ),
    ];
    for (name, body) in &broken {
        assert!(
            !problems(&load(name), body).is_empty(),
            "{name} accepted {body}"
        );
    }

    let spans = Spans::default();
    let active =
        serde_json::to_value(token::issue(LID, INST, IAT, &complimentary(None), &spans)).unwrap();
    assert_valid("token.claims.schema.json", &active);
    let revoked =
        serde_json::to_value(token::unknown_installation(LID, INST, IAT, &spans)).unwrap();
    assert_valid("token.claims.schema.json", &revoked);
    let claims_schema = load("token.claims.schema.json");
    let mut cases = Vec::new();
    let mut with_reason = active.clone();
    with_reason["reason"] = "refund".into();
    cases.push(with_reason);
    let mut without_reason = revoked.clone();
    without_reason.as_object_mut().unwrap().remove("reason");
    cases.push(without_reason);
    let mut with_term = revoked;
    with_term["term"] = "month".into();
    cases.push(with_term);
    for (field, value) in [
        ("v", json!(2)),
        ("plan", json!("free")),
        ("paid", json!("soon")),
        ("iat", json!(-1)),
    ] {
        let mut broken = active.clone();
        broken[field] = value;
        cases.push(broken);
    }
    let mut missing_paid = active.clone();
    missing_paid.as_object_mut().unwrap().remove("paid");
    cases.push(missing_paid);
    for case in &cases {
        assert!(
            !problems(&claims_schema, case).is_empty(),
            "accepted {case}"
        );
    }
    let mut extra = active;
    extra["later"] = json!(true);
    assert_valid("token.claims.schema.json", &extra);
}

#[test]
#[should_panic(expected = "does not know")]
fn the_validator_refuses_a_keyword_it_does_not_know() {
    problems(&json!({ "type": "string", "format": "email" }), &json!("x"));
}

#[test]
fn production_keys_hold_no_test_key_and_only_published_keys() {
    let production = load("production-keys.json");
    let published = KeyList::published().unwrap();
    for entry in production["keys"].as_array().unwrap() {
        let kid = entry["kid"].as_str().unwrap();
        assert!(!kid.starts_with("test-"), "{kid}");
        let listed = published
            .find(kid)
            .unwrap_or_else(|| panic!("{kid} is not published"));
        assert!(!listed.test, "{kid}");
        assert_eq!(
            serde_json::to_value(&listed.jwk).unwrap(),
            entry["jwk"],
            "{kid}"
        );
    }
}
