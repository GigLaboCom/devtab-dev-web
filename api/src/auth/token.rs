//! Supabase Auth's access tokens, checked here against the project's JWKS
//! (`<DEVTAB_SUPABASE_URL>/auth/v1/.well-known/jwks.json`) without asking
//! Supabase about each one.
//!
//! A token passes when a key of the set signed it, with ES256 or RS256 and the
//! key's own algorithm (a shared HS256 secret is never accepted), its `iss` is
//! the project's, `aud` is `authenticated`, `exp` has not passed by the API's
//! clock (with [`LEEWAY_S`] of slack), `sub` is a UUID and the session is not
//! anonymous.
//!
//! The set is kept for [`KEYS_TTL`]. A token naming a `kid` the set does not
//! have fetches it again (the project rotated its key), at most once per
//! [`MIN_REFETCH`], so that made-up `kid`s cannot flood Supabase. When the set
//! cannot be fetched, the keys already kept still work; with none that
//! matches, the answer is "unavailable", never "unauthorized".

use std::collections::HashMap;

use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk, KeyAlgorithm};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use time::{Duration, OffsetDateTime};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::clock::SharedClock;

/// How long a fetched key set is used before it is fetched again.
pub const KEYS_TTL: Duration = Duration::minutes(5);
/// The least time between two fetches of the set.
pub const MIN_REFETCH: Duration = Duration::seconds(30);
/// How long a fetch of the set may take.
pub const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
/// Clock skew allowed on `exp`, seconds.
pub const LEEWAY_S: i64 = 30;
/// The audience of a signed-in person's token.
pub const AUDIENCE: &str = "authenticated";

/// What the API takes from a token that passed. Nothing else is read from
/// it: whether the person may act comes from `app.users`.
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub exp: i64,
    #[serde(default)]
    pub is_anonymous: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("not a token")]
    Malformed,
    #[error("an algorithm this API does not accept")]
    Algorithm,
    #[error("a signing key the project's set does not have")]
    UnknownKey,
    #[error("the token does not pass: {0}")]
    Invalid(#[from] jsonwebtoken::errors::Error),
    #[error("the token has expired")]
    Expired,
    #[error("an anonymous session")]
    Anonymous,
    /// The set could not be fetched and no key kept from before matches.
    #[error("the project's signing keys cannot be fetched: {0}")]
    KeysUnavailable(String),
}

#[derive(Clone)]
struct Key {
    algorithm: Algorithm,
    decoding: DecodingKey,
}

#[derive(Default)]
struct KeySet {
    keys: HashMap<String, Key>,
    /// When the set was last fetched, or a fetch last failed.
    fetched_at: Option<OffsetDateTime>,
}

impl KeySet {
    fn younger_than(&self, span: Duration, now: OffsetDateTime) -> bool {
        self.fetched_at.is_some_and(|at| now - at < span)
    }
}

enum Source {
    Remote {
        http: reqwest::Client,
        url: String,
    },
    /// A set given once and never fetched (tests).
    Fixed,
}

/// Checks access tokens. One per process, shared through the state.
pub struct Verifier {
    source: Source,
    issuer: String,
    clock: SharedClock,
    keys: RwLock<KeySet>,
    /// One fetch at a time.
    fetching: Mutex<()>,
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verifier")
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

impl Verifier {
    /// Keys from `jwks_url`, fetched when the first token arrives.
    pub fn remote(
        jwks_url: String,
        issuer: String,
        clock: SharedClock,
    ) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .user_agent(concat!("devtab-api/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self::new(
            Source::Remote {
                http,
                url: jwks_url,
            },
            KeySet::default(),
            issuer,
            clock,
        ))
    }

    /// A fixed set, given as the JWKS's JSON, never fetched.
    pub fn fixed(jwks: &serde_json::Value, issuer: String, clock: SharedClock) -> Self {
        let keys = KeySet {
            keys: parse_jwks(jwks),
            fetched_at: None,
        };
        Self::new(Source::Fixed, keys, issuer, clock)
    }

    fn new(source: Source, keys: KeySet, issuer: String, clock: SharedClock) -> Self {
        Self {
            source,
            issuer,
            clock,
            keys: RwLock::new(keys),
            fetching: Mutex::new(()),
        }
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Checks `token` and returns what the API takes from it.
    pub async fn verify(&self, token: &str) -> Result<Claims, TokenError> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| TokenError::Malformed)?;
        if !matches!(header.alg, Algorithm::ES256 | Algorithm::RS256) {
            return Err(TokenError::Algorithm);
        }
        let kid = header.kid.ok_or(TokenError::UnknownKey)?;
        let key = self.key(&kid).await?;
        if key.algorithm != header.alg {
            return Err(TokenError::Algorithm);
        }

        let mut validation = Validation::new(key.algorithm);
        // `exp` is checked below against the API's clock, not the system's.
        validation.validate_exp = false;
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[AUDIENCE]);
        let claims = jsonwebtoken::decode::<Claims>(token, &key.decoding, &validation)?.claims;
        if claims.exp.saturating_add(LEEWAY_S) <= self.clock.now().unix_timestamp() {
            return Err(TokenError::Expired);
        }
        if claims.is_anonymous {
            return Err(TokenError::Anonymous);
        }
        Ok(claims)
    }

    async fn key(&self, kid: &str) -> Result<Key, TokenError> {
        let fixed = matches!(self.source, Source::Fixed);
        {
            let set = self.keys.read().await;
            if let Some(key) = set.keys.get(kid)
                && (fixed || set.younger_than(KEYS_TTL, self.clock.now()))
            {
                return Ok(key.clone());
            }
        }
        let Source::Remote { http, url } = &self.source else {
            return Err(TokenError::UnknownKey);
        };

        let _fetching = self.fetching.lock().await;
        // Whoever waited for another fetch looks at what it brought first.
        {
            let set = self.keys.read().await;
            let now = self.clock.now();
            if let Some(key) = set.keys.get(kid)
                && set.younger_than(KEYS_TTL, now)
            {
                return Ok(key.clone());
            }
            if set.younger_than(MIN_REFETCH, now) {
                return set.keys.get(kid).cloned().ok_or(TokenError::UnknownKey);
            }
        }

        let fetched = fetch_jwks(http, url).await;
        let mut set = self.keys.write().await;
        set.fetched_at = Some(self.clock.now());
        match fetched {
            Ok(keys) => {
                set.keys = keys;
                set.keys.get(kid).cloned().ok_or(TokenError::UnknownKey)
            }
            Err(err) => {
                tracing::warn!(error = %err, "fetching the project's JWKS failed");
                set.keys
                    .get(kid)
                    .cloned()
                    .ok_or_else(|| TokenError::KeysUnavailable(err.to_string()))
            }
        }
    }
}

async fn fetch_jwks(
    http: &reqwest::Client,
    url: &str,
) -> Result<HashMap<String, Key>, reqwest::Error> {
    let body: serde_json::Value = http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(parse_jwks(&body))
}

/// The keys of a set that have a `kid` and a kind this API accepts; the rest
/// (a symmetric key, another curve) are left out.
fn parse_jwks(body: &serde_json::Value) -> HashMap<String, Key> {
    let Some(keys) = body.get("keys").and_then(serde_json::Value::as_array) else {
        return HashMap::new();
    };
    keys.iter()
        .filter_map(|raw| {
            let jwk: Jwk = serde_json::from_value(raw.clone()).ok()?;
            let kid = jwk.common.key_id.clone()?;
            let algorithm = algorithm_of(&jwk)?;
            let decoding = DecodingKey::from_jwk(&jwk).ok()?;
            Some((
                kid,
                Key {
                    algorithm,
                    decoding,
                },
            ))
        })
        .collect()
}

/// ES256 for a P-256 key, RS256 for an RSA key; an `alg` the key states must
/// agree.
fn algorithm_of(jwk: &Jwk) -> Option<Algorithm> {
    let by_kind = match &jwk.algorithm {
        AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P256 => {
            Algorithm::ES256
        }
        AlgorithmParameters::RSA(_) => Algorithm::RS256,
        _ => return None,
    };
    match (jwk.common.key_algorithm, by_kind) {
        (None, _)
        | (Some(KeyAlgorithm::ES256), Algorithm::ES256)
        | (Some(KeyAlgorithm::RS256), Algorithm::RS256) => Some(by_kind),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_set_keeps_the_keys_it_accepts() {
        let set = json!({ "keys": [
            // P-256, stated as ES256.
            { "kty": "EC", "crv": "P-256", "kid": "ec", "alg": "ES256",
              "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
              "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" },
            // RSA, no alg stated.
            { "kty": "RSA", "kid": "rsa", "e": "AQAB",
              "n": "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw" },
            // The wrong alg for its kind, a symmetric key, another curve, no kid.
            { "kty": "EC", "crv": "P-256", "kid": "liar", "alg": "RS256",
              "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
              "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" },
            { "kty": "oct", "kid": "hmac", "k": "c2VjcmV0" },
            { "kty": "OKP", "crv": "Ed25519", "kid": "ed", "x": "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo" },
            { "kty": "EC", "crv": "P-256",
              "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
              "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" }
        ]});
        let keys = parse_jwks(&set);
        let mut kids: Vec<(&str, Algorithm)> = keys
            .iter()
            .map(|(kid, key)| (kid.as_str(), key.algorithm))
            .collect();
        kids.sort_by_key(|(kid, _)| *kid);
        assert_eq!(kids, [("ec", Algorithm::ES256), ("rsa", Algorithm::RS256)]);
        assert!(parse_jwks(&json!({ "nope": [] })).is_empty());
    }
}
