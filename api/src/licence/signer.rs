//! The signing key and the list of published public keys.
//!
//! The private key is a PKCS#8 PEM file on the server (`DEVTAB_SIGNING_KEY_FILE`,
//! id `DEVTAB_SIGNING_KID`), never in the repository, the environment, the
//! database or CI. The public keys are `config/signing-keys.json`, built into
//! the binary; the extension ships the same list. Before it serves, the API
//! checks that its private key is the published one with that id and that
//! the key's `sign_from` has come.
//!
//! No private key is ever in the repository, not even a test one. Until
//! production keys exist the published list is empty. In `dev` the API also
//! trusts the developer's own test keys, listed in the file
//! `DEVTAB_DEV_SIGNING_KEYS_FILE` (`script/api-dev` makes one on first use in
//! the ignored `.state/`); `staging` and `prod` refuse that setting and any
//! test key. Tests make their keys in memory.

use std::path::Path;

use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _, LineEnding};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::random_bytes;
use super::token::{Claims, Header, signing_input};
use crate::config::Env;

/// The published public keys, as built into this binary.
pub const PUBLISHED_KEYS: &str = include_str!("../../../config/signing-keys.json");
/// The id of every test key starts with this.
pub const TEST_KID_PREFIX: &str = "test-";

/// A P-256 public key as a JWK (the extension's `PublicKeyEntry.jwk`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Jwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub y: String,
}

impl Jwk {
    fn of(key: &VerifyingKey) -> Self {
        let point = key.to_sec1_point(false);
        let coordinate = |c: Option<&[u8]>| URL_SAFE_NO_PAD.encode(c.unwrap_or_default());
        Self {
            kty: "EC".to_owned(),
            crv: "P-256".to_owned(),
            x: coordinate(point.x().map(AsRef::as_ref)),
            y: coordinate(point.y().map(AsRef::as_ref)),
        }
    }

    pub fn verifying_key(&self) -> anyhow::Result<VerifyingKey> {
        anyhow::ensure!(self.kty == "EC" && self.crv == "P-256", "not a P-256 key");
        let x = URL_SAFE_NO_PAD
            .decode(&self.x)
            .context("x is not base64url")?;
        let y = URL_SAFE_NO_PAD
            .decode(&self.y)
            .context("y is not base64url")?;
        anyhow::ensure!(x.len() == 32 && y.len() == 32, "x and y must be 32 bytes");
        let mut sec1 = Vec::with_capacity(65);
        sec1.push(0x04);
        sec1.extend_from_slice(&x);
        sec1.extend_from_slice(&y);
        VerifyingKey::from_sec1_bytes(&sec1).context("not a point on P-256")
    }
}

/// One entry of `config/signing-keys.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedKey {
    pub kid: String,
    /// A test key: its private half is not secret.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub test: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub jwk: Jwk,
    /// The extension version the key first shipped in.
    pub shipped_in: Option<String>,
    /// RFC 3339: the server signs with the key from then on, not before.
    pub sign_from: Option<String>,
}

/// `config/signing-keys.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub keys: Vec<PublishedKey>,
}

fn is_kid(kid: &str) -> bool {
    (1..=32).contains(&kid.len())
        && kid
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl KeyList {
    /// Parses and checks a list: unique ids by the token's rule, points on
    /// the curve, test keys and only test keys named `test-…`.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let list: Self = serde_json::from_str(text).context("not a key list")?;
        for (i, key) in list.keys.iter().enumerate() {
            anyhow::ensure!(
                is_kid(&key.kid),
                "kid `{}` breaks [a-z0-9-]{{1,32}}",
                key.kid
            );
            anyhow::ensure!(
                list.keys[..i].iter().all(|k| k.kid != key.kid),
                "kid `{}` appears twice",
                key.kid
            );
            anyhow::ensure!(
                key.test == key.kid.starts_with(TEST_KID_PREFIX),
                "kid `{}`: a test key, and only a test key, starts with `{TEST_KID_PREFIX}`",
                key.kid
            );
            key.jwk
                .verifying_key()
                .with_context(|| format!("kid `{}`", key.kid))?;
            if let Some(from) = &key.sign_from {
                crate::clock::parse_rfc3339(from)
                    .with_context(|| format!("kid `{}`: sign_from is not RFC 3339", key.kid))?;
            }
        }
        Ok(list)
    }

    /// The list built into this binary: never a test key.
    pub fn published() -> anyhow::Result<Self> {
        let list = Self::parse(PUBLISHED_KEYS).context("config/signing-keys.json")?;
        if let Some(key) = list.keys.iter().find(|k| k.test) {
            anyhow::bail!(
                "config/signing-keys.json: `{}` is a test key; test keys are never published",
                key.kid
            );
        }
        Ok(list)
    }

    /// A developer's own test keys (`DEVTAB_DEV_SIGNING_KEYS_FILE`): only test keys.
    pub fn load_dev(path: &Path) -> anyhow::Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let list = Self::parse(&text).with_context(|| path.display().to_string())?;
        if let Some(key) = list.keys.iter().find(|k| !k.test) {
            anyhow::bail!("{}: `{}` is not a test key", path.display(), key.kid);
        }
        Ok(list)
    }

    /// This list and `other`, which must not repeat an id.
    pub fn merged(mut self, other: Self) -> anyhow::Result<Self> {
        for key in other.keys {
            anyhow::ensure!(
                self.find(&key.kid).is_none(),
                "kid `{}` appears twice",
                key.kid
            );
            self.keys.push(key);
        }
        Ok(self)
    }

    /// What the API trusts in `env`: the published list, plus in `dev` the
    /// developer's test keys when `dev_keys` names them. Outside `dev` a
    /// `dev_keys` file is refused.
    pub fn trusted(env: Env, dev_keys: Option<&Path>) -> anyhow::Result<Self> {
        let published = Self::published()?;
        match dev_keys {
            None => Ok(published),
            Some(path) if env.is_dev() => published.merged(Self::load_dev(path)?),
            Some(_) => anyhow::bail!("DEVTAB_DEV_SIGNING_KEYS_FILE is accepted only in dev"),
        }
    }

    pub fn find(&self, kid: &str) -> Option<&PublishedKey> {
        self.keys.iter().find(|k| k.kid == kid)
    }

    /// The keys a token is checked against.
    pub fn verifiers(&self) -> Vec<(String, VerifyingKey)> {
        self.keys
            .iter()
            .filter_map(|k| Some((k.kid.clone(), k.jwk.verifying_key().ok()?)))
            .collect()
    }
}

/// The key the server signs tokens with.
pub struct Signer {
    kid: String,
    key: SigningKey,
}

// The private key never reaches a log.
impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer")
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

impl Signer {
    pub fn from_pkcs8_pem(kid: &str, pem: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(is_kid(kid), "kid `{kid}` breaks [a-z0-9-]{{1,32}}");
        let key = SigningKey::from_pkcs8_pem(pem)
            .map_err(|err| anyhow::anyhow!("not a PKCS#8 P-256 private key: {err}"))?;
        Ok(Self {
            kid: kid.to_owned(),
            key,
        })
    }

    pub fn load(kid: &str, path: &Path) -> anyhow::Result<Self> {
        let pem = std::fs::read_to_string(path)
            .with_context(|| format!("reading the signing key {}", path.display()))?;
        Self::from_pkcs8_pem(kid, &pem).with_context(|| path.display().to_string())
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    pub fn jwk(&self) -> Jwk {
        Jwk::of(self.key.verifying_key())
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        *self.key.verifying_key()
    }

    /// The compact token: deterministic ECDSA (RFC 6979), so the same claims
    /// always give the same token.
    pub fn sign(&self, claims: &Claims) -> String {
        self.sign_input(&signing_input(&Header::new(&self.kid), claims))
    }

    /// `input` (`<header>.<payload>`, as sent) with its signature appended:
    /// for tests that need tokens the issuing code would never make.
    pub fn sign_input(&self, input: &str) -> String {
        let signature: Signature = self.key.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }
}

/// A new key pair: the private key as PKCS#8 PEM, and its entry for
/// `config/signing-keys.json`. A test key gets the id `test-<8 hex>` and
/// `sign_from` now; any other key's `sign_from` and `shipped_in` are left for
/// the owner to fill in when the extension ships it.
pub fn generate(
    kid: Option<&str>,
    test: bool,
    now: OffsetDateTime,
) -> anyhow::Result<(String, PublishedKey)> {
    let kid = match (kid, test) {
        (Some(kid), false) => {
            anyhow::ensure!(is_kid(kid), "kid `{kid}` breaks [a-z0-9-]{{1,32}}");
            anyhow::ensure!(
                !kid.starts_with(TEST_KID_PREFIX),
                "`{TEST_KID_PREFIX}…` is for test keys: use --test"
            );
            kid.to_owned()
        }
        (None, true) => {
            let [a, b, c, d] = random_bytes::<4>()?;
            format!("{TEST_KID_PREFIX}{a:02x}{b:02x}{c:02x}{d:02x}")
        }
        (Some(_), true) => anyhow::bail!("a test key gets its own id: drop --kid"),
        (None, false) => anyhow::bail!("name the key with --kid (p1, p2, …) or make a --test key"),
    };
    let key = loop {
        // Almost every 32 random bytes are a valid scalar.
        if let Ok(key) = SigningKey::from_slice(&random_bytes::<32>()?) {
            break key;
        }
    };
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|err| anyhow::anyhow!("encoding the private key: {err}"))?;
    let entry = PublishedKey {
        kid: kid.clone(),
        test,
        note: test.then(|| {
            "TEST KEY, made on this machine: development and tests only, never published; \
             staging and prod refuse it."
                .to_owned()
        }),
        jwk: Jwk::of(key.verifying_key()),
        shipped_in: None,
        sign_from: test.then(|| crate::clock::rfc3339(now)),
    };
    Ok((pem.to_string(), entry))
}

/// Whether `signer` may sign here and now: its id is published, the published
/// key is its own, `sign_from` has come (skipped with `emergency`), and a
/// test key signs in `dev` only.
pub fn check(
    signer: &Signer,
    keys: &KeyList,
    env: Env,
    now: OffsetDateTime,
    emergency: bool,
) -> Result<(), Vec<String>> {
    let kid = signer.kid();
    if !env.is_dev() && kid.starts_with(TEST_KID_PREFIX) {
        return Err(vec![format!("`{kid}` is a test key: refused outside dev")]);
    }
    let Some(entry) = keys.find(kid) else {
        let hint = if keys.keys.is_empty() {
            " (no key is published yet; in dev, script/api-dev makes a local test key)"
        } else {
            ""
        };
        return Err(vec![format!(
            "the signing key `{kid}` is not in config/signing-keys.json{hint}"
        )]);
    };
    let mut problems = Vec::new();
    let jwk = signer.jwk();
    if entry.jwk != jwk {
        problems.push(format!(
            "the private key in DEVTAB_SIGNING_KEY_FILE does not match the public key `{kid}` \
             in config/signing-keys.json"
        ));
    }
    if !env.is_dev() {
        let is_test_key = entry.test
            || kid.starts_with(TEST_KID_PREFIX)
            || keys.keys.iter().any(|k| k.test && k.jwk == jwk);
        if is_test_key {
            problems.push(format!("`{kid}` is a test key: refused outside dev"));
        }
    }
    if !emergency {
        match entry.sign_from.as_deref().map(crate::clock::parse_rfc3339) {
            None => problems.push(format!(
                "`{kid}` has no sign_from in config/signing-keys.json"
            )),
            Some(Err(_)) => problems.push(format!("`{kid}`: sign_from is not RFC 3339")),
            Some(Ok(from)) if from > now => problems.push(format!(
                "`{kid}` may sign only from {from} (sign_from in config/signing-keys.json)"
            )),
            Some(Ok(_)) => {}
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use super::*;
    use crate::licence::token::{self, Entitlement, Refusal, Spans, Standing};

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_795_392_000).unwrap()
    }

    fn key_pair(test: bool) -> (Signer, KeyList) {
        let (pem, entry) = if test {
            generate(None, true, now()).unwrap()
        } else {
            let (pem, mut entry) = generate(Some("p9"), false, now()).unwrap();
            entry.sign_from = Some("2026-01-01T00:00:00Z".to_owned());
            (pem, entry)
        };
        let signer = Signer::from_pkcs8_pem(&entry.kid, &pem).unwrap();
        let list = KeyList {
            note: None,
            keys: vec![entry],
        };
        (signer, list)
    }

    fn claims() -> Claims {
        let licence = Entitlement {
            standing: Standing::Active,
            paid: None,
            term: None,
            renews: false,
        };
        token::issue(
            "lic_0000000000000000000000TEST",
            "q3V2mZ0bTEST0000000000",
            now().unix_timestamp(),
            &licence,
            &Spans::default(),
        )
    }

    #[test]
    fn a_signed_token_has_three_base64url_parts_a_64_byte_signature_and_verifies() {
        let (signer, list) = key_pair(true);
        let token = signer.sign(&claims());
        assert!(!token.contains('='));
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(URL_SAFE_NO_PAD.decode(parts[2]).unwrap().len(), 64);
        let header: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header.as_object().unwrap().len(), 3);
        assert_eq!(header["kid"], signer.kid());
        let (_, verified) = token::verify(&token, &list.verifiers()).unwrap();
        assert_eq!(verified, serde_json::to_value(claims()).unwrap());
    }

    #[test]
    fn signing_is_deterministic() {
        let (signer, _) = key_pair(true);
        assert_eq!(signer.sign(&claims()), signer.sign(&claims()));
    }

    #[test]
    fn a_changed_byte_or_another_key_fails() {
        let (signer, list) = key_pair(true);
        let (_, unrelated) = key_pair(true);
        let token = signer.sign(&claims());
        let mut bytes = token.clone().into_bytes();
        let tail = bytes.len() - 2;
        bytes[tail] = if bytes[tail] == b'A' { b'B' } else { b'A' };
        let broken = String::from_utf8(bytes).unwrap();
        assert_eq!(
            token::verify(&broken, &list.verifiers()),
            Err(Refusal::Signature)
        );
        let renamed: Vec<_> = unrelated
            .verifiers()
            .into_iter()
            .map(|(_, key)| (signer.kid().to_owned(), key))
            .collect();
        assert_eq!(token::verify(&token, &renamed), Err(Refusal::Signature));
        assert_eq!(
            token::verify(&token, &unrelated.verifiers()),
            Err(Refusal::UnknownKey)
        );
    }

    #[test]
    fn the_extension_order_of_refusals() {
        let (signer, list) = key_pair(true);
        let keys = list.verifiers();
        let token = signer.sign(&claims());
        let parts: Vec<&str> = token.split('.').collect();
        let none = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","kid":"x","typ":"devtab-licence"}"#);
        assert_eq!(
            token::verify(&format!("{none}.{}.", parts[1]), &keys),
            Err(Refusal::Unsupported)
        );
        let short_sig = URL_SAFE_NO_PAD.encode([1_u8; 63]);
        assert_eq!(
            token::verify(&format!("{}.{}.{short_sig}", parts[0], parts[1]), &keys),
            Err(Refusal::Malformed)
        );
        assert_eq!(token::verify("a.b", &keys), Err(Refusal::Malformed));
        assert_eq!(
            token::verify(&"a".repeat(4097), &keys),
            Err(Refusal::Malformed)
        );
        assert_eq!(
            token::verify(&format!("{token}="), &keys),
            Err(Refusal::Malformed)
        );
    }

    #[test]
    fn a_pem_round_trip_keeps_the_key() {
        let (pem, entry) = generate(None, true, now()).unwrap();
        assert!(pem.starts_with(concat!("-----BEGIN", " PRIV", "ATE KEY-----")));
        assert!(entry.kid.starts_with("test-") && entry.kid.len() == 13);
        assert!(entry.test);
        let signer = Signer::from_pkcs8_pem(&entry.kid, &pem).unwrap();
        assert_eq!(signer.jwk(), entry.jwk);
        assert_eq!(entry.jwk.x.len(), 43);
        assert!(!format!("{signer:?}").contains("BEGIN"));
    }

    #[test]
    fn generate_names_keys_by_their_kind() {
        assert!(generate(Some("test-x"), false, now()).is_err());
        assert!(generate(Some("p1"), true, now()).is_err());
        assert!(generate(None, false, now()).is_err());
        let (_, entry) = generate(Some("p1"), false, now()).unwrap();
        assert_eq!(
            (entry.sign_from, entry.shipped_in, entry.test),
            (None, None, false)
        );
    }

    #[test]
    fn the_check_passes_for_the_published_key_and_its_time() {
        let (signer, list) = key_pair(false);
        assert_eq!(check(&signer, &list, Env::Prod, now(), false), Ok(()));
        let (test_signer, test_list) = key_pair(true);
        assert_eq!(
            check(&test_signer, &test_list, Env::Dev, now(), false),
            Ok(())
        );
    }

    #[test]
    fn a_mismatched_key_is_refused() {
        let (signer, _) = key_pair(false);
        let (_, mut other) = key_pair(false);
        other.keys[0].kid = signer.kid().to_owned();
        let problems = check(&signer, &other, Env::Dev, now(), false).unwrap_err();
        assert!(problems[0].contains("does not match"), "{problems:?}");
        let (_, unrelated) = key_pair(true);
        let problems = check(&signer, &unrelated, Env::Dev, now(), false).unwrap_err();
        assert!(problems[0].contains("not in config/signing-keys.json"));
    }

    #[test]
    fn a_test_key_is_refused_outside_dev() {
        let (signer, list) = key_pair(true);
        for env in [Env::Staging, Env::Prod] {
            let problems = check(&signer, &list, env, now(), false).unwrap_err();
            assert!(problems[0].contains("test key"), "{problems:?}");
        }
        // Also when the same key is published again under another name.
        let mut renamed = list.clone();
        let mut entry = renamed.keys[0].clone();
        entry.kid = "p7".to_owned();
        entry.test = false;
        renamed.keys.push(entry);
        let as_p7 = Signer {
            kid: "p7".to_owned(),
            key: signer.key.clone(),
        };
        assert!(check(&as_p7, &renamed, Env::Prod, now(), false).is_err());
    }

    #[test]
    fn sign_from_must_be_set_and_reached_unless_emergency() {
        let (signer, mut list) = key_pair(false);
        list.keys[0].sign_from = Some(crate::clock::rfc3339(now() + Duration::days(1)));
        assert!(check(&signer, &list, Env::Prod, now(), false).is_err());
        assert_eq!(check(&signer, &list, Env::Prod, now(), true), Ok(()));
        list.keys[0].sign_from = None;
        assert!(check(&signer, &list, Env::Prod, now(), false).is_err());
    }

    #[test]
    fn the_list_rules() {
        let (_, list) = key_pair(true);
        let text = serde_json::to_string(&list).unwrap();
        assert!(KeyList::parse(&text).is_ok());
        let mut twice = list.clone();
        twice.keys.push(list.keys[0].clone());
        assert!(KeyList::parse(&serde_json::to_string(&twice).unwrap()).is_err());
        let mut unmarked = list.clone();
        unmarked.keys[0].test = false;
        assert!(KeyList::parse(&serde_json::to_string(&unmarked).unwrap()).is_err());
        let mut off_curve = list;
        off_curve.keys[0].jwk.y = off_curve.keys[0].jwk.x.clone();
        assert!(KeyList::parse(&serde_json::to_string(&off_curve).unwrap()).is_err());
    }

    #[test]
    fn the_published_list_parses_and_holds_no_test_key() {
        let list = KeyList::published().unwrap();
        assert!(list.keys.iter().all(|k| !k.test));
    }

    #[test]
    fn the_dev_list_is_only_test_keys_and_only_in_dev() {
        let dir = std::env::temp_dir().join(format!("devtab-signer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_, test_list) = key_pair(true);
        let path = dir.join("keys.json");
        std::fs::write(&path, serde_json::to_string(&test_list).unwrap()).unwrap();
        let trusted = KeyList::trusted(Env::Dev, Some(&path)).unwrap();
        assert!(trusted.find(&test_list.keys[0].kid).is_some());
        for env in [Env::Staging, Env::Prod] {
            let err = KeyList::trusted(env, Some(&path)).unwrap_err();
            assert!(err.to_string().contains("only in dev"), "{err}");
        }
        let (_, production) = key_pair(false);
        std::fs::write(&path, serde_json::to_string(&production).unwrap()).unwrap();
        assert!(
            KeyList::load_dev(&path).is_err(),
            "a production key in the dev list"
        );
        assert!(test_list.clone().merged(test_list).is_err(), "an id twice");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_published_list_says_so() {
        let (signer, _) = key_pair(true);
        let empty = KeyList {
            note: None,
            keys: Vec::new(),
        };
        let problems = check(&signer, &empty, Env::Dev, now(), false).unwrap_err();
        assert!(
            problems[0].contains("no key is published yet"),
            "{problems:?}"
        );
    }
}
