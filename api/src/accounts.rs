//! The site's accounts at Supabase Auth, as far as a purchase without an
//! account needs them: inviting the buyer of a guest checkout by mail.
//!
//! Finding a user by address is not done here: the API reads it from the
//! database through `app.user_id_by_email`, a narrow security definer
//! function granted to the API role alone (`store::users::id_by_email`). The
//! API role cannot read `auth`, and schema `app` is closed to the Data API, so
//! nothing else is opened; Supabase Auth's admin API has no lookup by exact
//! address (its list of users would have to be paged through).
//!
//! Inviting is `POST <auth>/invite` with the project's secret key: Supabase
//! Auth creates the user, unconfirmed, and sends the invite mail
//! (`supabase/templates/invite.html`). It invites an unconfirmed address
//! again each time it is asked, so the caller looks the address up first
//! and never asks for an address that has an account.
//!
//! The secret key is a file the setting names (`DEVTAB_SUPABASE_SECRET_KEY_FILE`),
//! read at start, never logged; the address of Auth is a setting too.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::StatusCode;
use serde_json::Value;
use uuid::Uuid;

/// Where Supabase Auth's admin API is, and the file of the key that may use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSettings {
    /// Supabase Auth as the API reaches it: `<DEVTAB_SUPABASE_URL>/auth/v1`
    /// unless `DEVTAB_SUPABASE_AUTH_URL` names another address.
    pub auth_url: String,
    /// `DEVTAB_SUPABASE_SECRET_KEY_FILE`.
    pub secret_key_file: PathBuf,
}

/// The project's secret key (`sb_secret_…`), or its legacy `service_role`
/// key (a JWT). A publishable or anon key is refused: it cannot invite.
#[derive(Clone)]
pub struct SecretKey(String);

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey(<redacted>)")
    }
}

impl SecretKey {
    /// Checks a key as the file holds it (surrounding white space dropped).
    pub fn parse(text: &str) -> Result<Self, String> {
        let key = text.trim();
        let printable = !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
        if !printable {
            return Err("not a Supabase key: empty, or characters a key has not".to_owned());
        }
        if key.starts_with("sb_secret_") {
            return Ok(Self(key.to_owned()));
        }
        if key.starts_with("sb_publishable_") {
            return Err(
                "a publishable key: guest checkout needs the project's secret key (sb_secret_…)"
                    .to_owned(),
            );
        }
        if key.starts_with("eyJ") && key.split('.').count() == 3 {
            // A legacy key: only its `service_role` may use the admin API.
            let role = key
                .split('.')
                .nth(1)
                .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|claims| {
                    claims
                        .get("role")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            return match role.as_deref() {
                Some("service_role") => Ok(Self(key.to_owned())),
                _ => Err("a legacy key that is not the service_role key".to_owned()),
            };
        }
        Err("not a Supabase secret key (sb_secret_…) or service_role key".to_owned())
    }

    /// Reads and checks the key file.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the Supabase secret key from {}", path.display()))?;
        Self::parse(&text).map_err(|problem| anyhow!("{}: {problem}", path.display()))
    }
}

/// Why an invitation did not happen.
#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    /// The address has an account already (someone else made it in the
    /// meantime): look it up again.
    #[error("the address has an account already")]
    Exists,
    /// Supabase Auth could not be asked, or failed: worth trying later.
    #[error("Supabase Auth could not be reached: {0}")]
    Unavailable(String),
    /// Supabase Auth refused (the key, the address, a setting): asking again
    /// does not help.
    #[error("Supabase Auth refused the invitation: {0}")]
    Refused(String),
}

/// What an invited user's account starts with: its user metadata, which the
/// invite mail and the site's callback read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation<'a> {
    /// `en` or `ru`: the mail's language, and the site's after the link.
    pub locale: &'a str,
    /// The checkout the account comes from: the site's callback sends the
    /// person to its page.
    pub checkout: Uuid,
    /// A team plan (the mail says "Team" rather than "Pro").
    pub team: bool,
}

/// Supabase Auth's admin API with the project's secret key.
#[derive(Debug, Clone)]
pub struct Accounts {
    http: reqwest::Client,
    auth_url: String,
    key: SecretKey,
    /// Where Supabase Auth may send the browser after the invite's link: the
    /// site's callback. The invite mail links the callback itself (with the
    /// project's Site URL), so this matters only to a template that uses
    /// `{{ .ConfirmationURL }}`.
    redirect_to: String,
}

/// How long one request to Supabase Auth may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The largest answer read from Supabase Auth.
const MAX_ANSWER_BYTES: usize = 256 * 1024;

impl Accounts {
    /// Reads the key from its file and builds the client; `site_url` is the
    /// site's address (`DEVTAB_SITE_URL`).
    pub fn connect(settings: &AccountSettings, site_url: &str) -> anyhow::Result<Self> {
        let key = SecretKey::read(&settings.secret_key_file)?;
        Self::with_key(&settings.auth_url, key, site_url)
    }

    /// The client with a key already read.
    pub fn with_key(auth_url: &str, key: SecretKey, site_url: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!("devtab-api/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building the HTTP client")?;
        Ok(Self {
            http,
            auth_url: auth_url.trim_end_matches('/').to_owned(),
            key,
            redirect_to: format!("{}/auth/callback/", site_url.trim_end_matches('/')),
        })
    }

    /// Invites `email` (trimmed and lower case already): Supabase Auth creates
    /// the user and sends the invite mail. The new user's id.
    pub async fn invite(
        &self,
        email: &str,
        invitation: &Invitation<'_>,
    ) -> Result<Uuid, AccountError> {
        let body = serde_json::json!({
            "email": email,
            "data": {
                "locale": invitation.locale,
                "app": "devtab",
                "source": "purchase",
                "checkout": invitation.checkout.to_string(),
                "team": invitation.team,
            },
        });
        let response = self
            .http
            .post(format!("{}/invite", self.auth_url))
            .query(&[("redirect_to", self.redirect_to.as_str())])
            // As supabase-js sends it: the gateway takes the key as `apikey`,
            // Auth itself as the bearer.
            .header("apikey", &self.key.0)
            .bearer_auth(&self.key.0)
            .json(&body)
            .send()
            .await
            .map_err(|err| AccountError::Unavailable(without_url(err)))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| AccountError::Unavailable(without_url(err)))?;
        if bytes.len() > MAX_ANSWER_BYTES {
            return Err(AccountError::Unavailable(
                "Supabase Auth's answer is too large".to_owned(),
            ));
        }
        let answer: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if status.is_success() {
            return answer
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or_else(|| {
                    AccountError::Unavailable("Supabase Auth's answer has no user id".to_owned())
                });
        }
        // `error_code` (or `code`, by the API version), never the message,
        // which may name the address.
        let code = answer
            .get("error_code")
            .or_else(|| answer.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_owned();
        if matches!(code.as_str(), "email_exists" | "user_already_exists") {
            return Err(AccountError::Exists);
        }
        let what = format!("{} {code}", status.as_u16());
        if status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::REQUEST_TIMEOUT
            || status.is_server_error()
        {
            Err(AccountError::Unavailable(what))
        } else {
            Err(AccountError::Refused(what))
        }
    }
}

/// A request's error for the log, without its address's query (which holds
/// no secret, but keeps lines short and alike).
fn without_url(err: reqwest::Error) -> String {
    let mut text = err.without_url().to_string();
    text.truncate(300);
    text
}

/// An address as the buyer typed it at the provider, as accounts are looked
/// up and made: surrounding spaces dropped, lower case. Nothing else: no dots
/// or `+tags` removed, so that the account is the one the person signs in to
/// with the same address.
pub fn normal_email(raw: &str) -> Option<String> {
    let email = raw.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let ok = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && !domain.contains('@');
    ok.then_some(email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_secret_or_service_role_key_is_taken() {
        assert!(SecretKey::parse(" sb_secret_abc-DEF_123 \n").is_ok());
        let publishable = SecretKey::parse("sb_publishable_abc").unwrap_err();
        assert!(publishable.contains("publishable"), "{publishable}");
        let part = |v: &Value| URL_SAFE_NO_PAD.encode(v.to_string().as_bytes());
        let legacy = |role: &str| {
            format!(
                "{}.{}.sig",
                part(&serde_json::json!({"alg": "HS256"})),
                part(&serde_json::json!({"role": role}))
            )
        };
        assert!(SecretKey::parse(&legacy("service_role")).is_ok());
        assert!(SecretKey::parse(&legacy("anon")).is_err());
        assert!(SecretKey::parse("").is_err());
        assert!(SecretKey::parse("rk_test_123").is_err());
        assert!(SecretKey::parse("sb_secret_a b").is_err());
        let key = SecretKey::parse("sb_secret_hidden").unwrap();
        assert!(!format!("{key:?}").contains("hidden"));
    }

    #[test]
    fn an_address_is_trimmed_and_lower_cased_and_nothing_else() {
        assert_eq!(
            normal_email("  First.Last+Tag@GMail.com ").as_deref(),
            Some("first.last+tag@gmail.com"),
            "dots and tags stay: the account is the address the person signs in with"
        );
        for bad in [
            "",
            "no-at",
            "@example.invalid",
            "a@b",
            "a b@example.invalid",
            "a@.x",
        ] {
            assert_eq!(normal_email(bad), None, "{bad}");
        }
    }
}
