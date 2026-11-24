//! Installation secrets: 32 random bytes in base64url without padding (43
//! characters), handed to the extension once at activation. Only the
//! SHA-256 of the secret as sent is stored, and it is compared in constant
//! time.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;

use super::random_bytes;

/// A freshly made secret: sent once, then only its hash remains.
pub struct NewSecret {
    pub secret: String,
    pub hash: [u8; 32],
}

// The secret must never reach a log through `{:?}`.
impl std::fmt::Debug for NewSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NewSecret(..)")
    }
}

pub fn generate() -> anyhow::Result<NewSecret> {
    let secret = URL_SAFE_NO_PAD.encode(random_bytes::<32>()?);
    let hash = hash(&secret);
    Ok(NewSecret { secret, hash })
}

/// SHA-256 of a secret as the extension sends it.
pub fn hash(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

/// Whether `secret` is the one whose hash is `stored`, in constant time.
pub fn matches(secret: &str, stored: &[u8]) -> bool {
    bool::from(hash(secret).as_slice().ct_eq(stored))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::licence::wire;

    #[test]
    fn a_secret_is_43_base64url_characters_and_matches_its_hash() {
        let new = generate().unwrap();
        assert_eq!(new.secret.len(), 43);
        assert!(wire::is_secret(&new.secret));
        assert!(!new.secret.contains('='));
        assert!(matches(&new.secret, &new.hash));
        assert!(!matches(&generate().unwrap().secret, &new.hash));
        assert!(!matches(&new.secret, &new.hash[..31]), "a short hash");
        assert!(!format!("{new:?}").contains(&new.secret));
    }
}
