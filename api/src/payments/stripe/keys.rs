//! Stripe's two secrets, each read from the file the settings name, checked
//! against the mode, and never printed: the restricted API key and the
//! webhook destination's signing secret.

use std::fmt;
use std::path::Path;

use anyhow::{Context as _, anyhow};

use super::StripeMode;

/// A restricted API key, `rk_test_…` in a sandbox and `rk_live_…` in live
/// mode. A secret key (`sk_…`) is refused: a restricted key carries only the
/// permissions the adapter needs.
#[derive(Clone)]
pub struct ApiKey(String);

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

impl ApiKey {
    /// Checks a key as the file holds it (surrounding white space dropped).
    pub fn parse(text: &str, mode: StripeMode) -> Result<Self, String> {
        let key = text.trim();
        let prefix = match mode {
            StripeMode::Sandbox => "rk_test_",
            StripeMode::Live => "rk_live_",
        };
        if let Some(rest) = key.strip_prefix(prefix) {
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Ok(Self(key.to_owned()));
            }
            return Err(format!(
                "the key after {prefix} has characters a Stripe key has not"
            ));
        }
        Err(if key.starts_with("sk_") {
            "a secret key (sk_…): create a restricted key (rk_…) with only the permissions the \
             adapter needs"
                .to_owned()
        } else if key.starts_with("rk_") {
            format!(
                "a restricted key of the other mode: DEVTAB_PAYMENTS_ENV={mode} needs {prefix}…"
            )
        } else {
            format!("not a Stripe restricted key: DEVTAB_PAYMENTS_ENV={mode} needs {prefix}…")
        })
    }

    /// Reads and checks the key file.
    pub fn read(path: &Path, mode: StripeMode) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the Stripe API key from {}", path.display()))?;
        Self::parse(&text, mode).map_err(|problem| anyhow!("{}: {problem}", path.display()))
    }

    pub(super) fn expose(&self) -> &str {
        &self.0
    }
}

/// The signing secret of a webhook destination, `whsec_…`; each destination
/// has its own, the sandbox's and live mode's included.
#[derive(Clone)]
pub struct WebhookSecret(String);

impl fmt::Debug for WebhookSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WebhookSecret(<redacted>)")
    }
}

impl WebhookSecret {
    /// Checks a secret as the file holds it (surrounding white space dropped).
    pub fn parse(text: &str) -> Result<Self, String> {
        let secret = text.trim();
        match secret.strip_prefix("whsec_") {
            Some(rest)
                if !rest.is_empty()
                    && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
            {
                Ok(Self(secret.to_owned()))
            }
            _ => Err("not a Stripe webhook signing secret: it starts with whsec_".to_owned()),
        }
    }

    /// Reads and checks the secret file.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!("reading the Stripe webhook secret from {}", path.display())
        })?;
        Self::parse(&text).map_err(|problem| anyhow!("{}: {problem}", path.display()))
    }

    pub(super) fn expose(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restricted_key_must_match_the_mode() {
        let key = ApiKey::parse("rk_test_abc123\n", StripeMode::Sandbox).unwrap();
        assert_eq!(key.expose(), "rk_test_abc123");
        assert!(ApiKey::parse("rk_live_abc123", StripeMode::Live).is_ok());
        let other = ApiKey::parse("rk_live_abc123", StripeMode::Sandbox).unwrap_err();
        assert!(other.contains("other mode"), "{other}");
        let secret = ApiKey::parse("sk_test_abc123", StripeMode::Sandbox).unwrap_err();
        assert!(secret.contains("restricted key"), "{secret}");
        assert!(ApiKey::parse("rk_test_", StripeMode::Sandbox).is_err());
        assert!(ApiKey::parse("rk_test_a b", StripeMode::Sandbox).is_err());
        assert!(ApiKey::parse("pk_test_abc", StripeMode::Sandbox).is_err());
    }

    #[test]
    fn a_webhook_secret_starts_with_whsec() {
        let secret = WebhookSecret::parse(" whsec_abc123 \n").unwrap();
        assert_eq!(secret.expose(), b"whsec_abc123");
        assert!(WebhookSecret::parse("whsec_").is_err());
        assert!(WebhookSecret::parse("rk_test_abc").is_err());
    }

    #[test]
    fn neither_is_printed() {
        let key = ApiKey::parse("rk_test_hunter2", StripeMode::Sandbox).unwrap();
        let secret = WebhookSecret::parse("whsec_hunter2").unwrap();
        assert!(!format!("{key:?} {secret:?}").contains("hunter2"));
    }

    #[test]
    fn reading_names_the_file() {
        let err = ApiKey::read(Path::new("/nonexistent/key"), StripeMode::Sandbox).unwrap_err();
        assert!(format!("{err:#}").contains("/nonexistent/key"));
    }
}
