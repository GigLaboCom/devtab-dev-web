//! Licence keys and licence ids.
//!
//! A key is `DTD-` and 20 Crockford base32 characters in groups of four
//! (`DTD-XXXX-XXXX-XXXX-XXXX-XXXX`): 100 random bits. It is compared in its
//! canonical form (upper case, no spaces or hyphens, `O` read as `0`, `I` and
//! `L` as `1`) and stored only as the SHA-256 of that form. Anything that
//! does not reach the canonical pattern is simply an unknown key: the format
//! is not revealed.
//!
//! A licence id is `lic_` and 26 Crockford base32 characters of 16 random
//! bytes; it carries no time and no order.

use sha2::{Digest as _, Sha256};

use super::random_bytes;

/// Crockford's base32 alphabet: no `I`, `L`, `O`, `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const PREFIX: &str = "DTD";
/// Characters after the prefix.
const KEY_CHARS: usize = 20;

/// A freshly made key: shown once, then only its hash and hint remain.
pub struct NewKey {
    /// `DTD-XXXX-XXXX-XXXX-XXXX-XXXX`.
    pub display: String,
    pub hash: [u8; 32],
    pub hint: String,
}

// The key must never reach a log through `{:?}`.
impl std::fmt::Debug for NewKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewKey")
            .field("hint", &self.hint)
            .finish_non_exhaustive()
    }
}

/// The first `chars` × 5 bits of `bytes` in Crockford base32.
fn crockford(bytes: &[u8], chars: usize) -> String {
    (0..chars)
        .map(|i| {
            let value = (0..5).fold(0_usize, |acc, j| {
                let bit = i * 5 + j;
                let set = bytes
                    .get(bit / 8)
                    .is_some_and(|byte| byte & (0x80 >> (bit % 8)) != 0);
                (acc << 1) | usize::from(set)
            });
            char::from(ALPHABET[value])
        })
        .collect()
}

/// A new random key.
pub fn generate() -> anyhow::Result<NewKey> {
    // 13 bytes are 104 bits; the key uses the first 100.
    let body = crockford(&random_bytes::<13>()?, KEY_CHARS);
    let canonical = format!("{PREFIX}{body}");
    let groups: Vec<&str> = (0..KEY_CHARS / 4)
        .map(|g| &body[g * 4..g * 4 + 4])
        .collect();
    Ok(NewKey {
        display: format!("{PREFIX}-{}", groups.join("-")),
        hash: hash(&canonical),
        hint: hint(&canonical).to_owned(),
    })
}

/// The canonical form of a key as typed or pasted, or `None` when it is not
/// a key of this format.
pub fn canonical(raw: &str) -> Option<String> {
    let canonical: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect();
    let body = canonical.strip_prefix(PREFIX)?;
    (body.len() == KEY_CHARS && body.bytes().all(|b| ALPHABET.contains(&b))).then_some(canonical)
}

/// SHA-256 of a canonical key.
pub fn hash(canonical: &str) -> [u8; 32] {
    Sha256::digest(canonical.as_bytes()).into()
}

/// The last four characters of a canonical key.
pub fn hint(canonical: &str) -> &str {
    canonical
        .get(canonical.len().saturating_sub(4)..)
        .unwrap_or("")
}

/// A new random licence id.
pub fn new_lid() -> anyhow::Result<String> {
    Ok(format!("lic_{}", crockford(&random_bytes::<16>()?, 26)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_alphabet(text: &str) -> bool {
        text.bytes().all(|b| ALPHABET.contains(&b))
    }

    #[test]
    fn a_new_key_has_the_format_and_its_hash_and_hint() {
        for _ in 0..200 {
            let key = generate().unwrap();
            assert_eq!(key.display.len(), 28, "{}", key.display);
            let parts: Vec<&str> = key.display.split('-').collect();
            assert_eq!(parts[0], "DTD");
            assert_eq!(parts.len(), 6);
            assert!(parts[1..].iter().all(|p| p.len() == 4 && in_alphabet(p)));
            let canonical = canonical(&key.display).unwrap();
            assert_eq!(key.hash, hash(&canonical));
            assert_eq!(key.hint, &key.display[24..]);
        }
    }

    #[test]
    fn keys_differ() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert_ne!(a.display, b.display);
    }

    #[test]
    fn the_last_crockford_character_carries_four_random_bits_and_a_zero() {
        // 100 bits of 104: the twentieth character is the 96th–100th bit.
        assert_eq!(crockford(&[0xff; 13], 20), "Z".repeat(20));
        assert_eq!(crockford(&[0; 13], 20), "0".repeat(20));
        assert_eq!(crockford(&[0b1000_0100, 0b0010_0000], 3), "GGG");
        assert_eq!(crockford(&[0x01, 0x23, 0x45], 4), "04HM");
    }

    #[test]
    fn canonicalisation_ignores_case_spaces_hyphens_and_reads_o_i_l() {
        let canonical_form = "DTD0123456789ABCDEFGHJK";
        for typed in [
            "DTD-0123-4567-89AB-CDEF-GHJK",
            "dtd-0123-4567-89ab-cdef-ghjk",
            " DTD 0123 4567 89AB CDEF GHJK ",
            "DTD-O123-4567-89AB-CDEF-GHJK",
            "dtd0123456789abcdefghjk",
        ] {
            assert_eq!(canonical(typed).as_deref(), Some(canonical_form), "{typed}");
        }
        assert_eq!(
            canonical("DTD-I1L1-0000-0000-0000-0000").as_deref(),
            Some("DTD11110000000000000000")
        );
    }

    #[test]
    fn a_wrong_format_is_an_unknown_key() {
        for typed in [
            "DTD-0123-4567-89AB-CDEF-GHJ",    // 19 characters
            "DTD-0123-4567-89AB-CDEF-GHJKM",  // 21
            "DTX-0123-4567-89AB-CDEF-GHJK",   // prefix
            "DTD-0123-4567-89AB-CDEF-GHJU",   // U is not Crockford
            "DTD-0123-4567-89AB-CDEF-GH*K",   // punctuation
            "lic_0123456789ABCDEFGHJKMNPQRS", // a licence id
            "",
        ] {
            assert_eq!(canonical(typed), None, "{typed}");
        }
    }

    #[test]
    fn the_hash_is_sha256_of_the_canonical_key() {
        // printf 'DTD0123456789ABCDEFGHJK' | shasum -a 256
        let expected = "7ba322c841968a09e973e3b4dc52e2555566c68019bc4347c8d16f0e6b895e23";
        let got = hash("DTD0123456789ABCDEFGHJK")
            .iter()
            .fold(String::new(), |mut hex, b| {
                use std::fmt::Write as _;
                let _ = write!(hex, "{b:02x}");
                hex
            });
        assert_eq!(got, expected);
    }

    #[test]
    fn a_licence_id_has_its_pattern() {
        for _ in 0..100 {
            let lid = new_lid().unwrap();
            let body = lid.strip_prefix("lic_").unwrap();
            assert_eq!(body.len(), 26);
            assert!(in_alphabet(body), "{lid}");
        }
    }
}
