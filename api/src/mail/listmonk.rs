//! listmonk as a [`Mailer`]: its transactional API, `POST /api/tx`, with a
//! template of the instance for each kind and language
//! (`DEVTAB_MAIL_LISTMONK_TEMPLATES`, which `admin mail templates` prints),
//! HTTP basic auth with an API user and its token (the token is a file), and
//! the sender `DEVTAB_MAIL_FROM` when set (else the instance's default).
//!
//! listmonk's default transactional mode sends only to a subscriber of the
//! instance and refuses any other address. The owner's other products do it
//! two ways, and so does this:
//!
//! - with `DEVTAB_MAIL_LISTMONK_LIST_ID`, the recipient is first made a
//!   subscriber of that (private, single opt-in) list, preconfirmed, as
//!   `subscribeAndSend` does in their web backends; an existing subscriber is
//!   left as it is (create-only: `409` means it is there);
//! - without it, the mail goes in `external` mode (`subscriber_mode:
//!   external`), which needs no subscriber at all, as their one-to-one mails
//!   do. Only a listmonk that knows the mode takes it: v6.2.0 does, v5.1.0
//!   ignores it and refuses an address that is not a subscriber (both tried
//!   locally); with an older instance, set the list.
//!
//! Either way a template reads only `{{ .Tx.Data.* }}`, never
//! `{{ .Subscriber.* }}` (empty in `external` mode): everything it needs is
//! in the data.
//!
//! A refusal (4xx other than 408 and 429) is permanent; the network, 408, 429
//! and 5xx are worth trying again. The log names the status, never an
//! address.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use reqwest::StatusCode;
use serde_json::{Value, json};

use super::{LOCALES, Mail, MailError, MailFuture, MailKind, Mailer};

/// listmonk's settings, from the environment (`config.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListmonkSettings {
    /// `DEVTAB_MAIL_LISTMONK_URL`: the instance, `https://<host>`.
    pub url: String,
    /// `DEVTAB_MAIL_LISTMONK_API_USER`.
    pub api_user: String,
    /// `DEVTAB_MAIL_LISTMONK_API_TOKEN_FILE`: the API user's token.
    pub token_file: PathBuf,
    /// `DEVTAB_MAIL_LISTMONK_TEMPLATES`: the instance's template id of each
    /// `<kind>.<locale>`. A kind without one is refused when it is sent.
    pub templates: BTreeMap<String, i64>,
    /// `DEVTAB_MAIL_LISTMONK_LIST_ID`: the list recipients join before a
    /// mail; none: `external` mode.
    pub list_id: Option<i64>,
    /// `DEVTAB_MAIL_FROM`: `Name <address>`, else the instance's default.
    pub from: Option<String>,
}

impl ListmonkSettings {
    /// The `<kind>.<locale>` pairs without a template id.
    pub fn missing_templates(&self) -> Vec<String> {
        MailKind::ALL
            .iter()
            .flat_map(|kind| LOCALES.iter().map(move |locale| format!("{kind}.{locale}")))
            .filter(|key| !self.templates.contains_key(key))
            .collect()
    }
}

/// `DEVTAB_MAIL_LISTMONK_TEMPLATES`: `<kind>.<locale>=<id>`, comma-separated.
pub fn parse_templates(text: &str) -> Result<BTreeMap<String, i64>, String> {
    let mut map = BTreeMap::new();
    for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (key, id) = entry
            .split_once('=')
            .ok_or_else(|| format!("`{entry}` is not `<kind>.<locale>=<id>`"))?;
        let (kind, locale) = key
            .trim()
            .rsplit_once('.')
            .ok_or_else(|| format!("`{key}` is not `<kind>.<locale>`"))?;
        if MailKind::parse(kind).is_none() {
            return Err(format!("`{kind}` is not a kind of mail"));
        }
        if !LOCALES.contains(&locale) {
            return Err(format!("`{locale}` is not one of {}", LOCALES.join(", ")));
        }
        let id: i64 = id
            .trim()
            .parse()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| format!("`{id}` is not a template id"))?;
        map.insert(format!("{kind}.{locale}"), id);
    }
    Ok(map)
}

/// The API user's token, read from its file; never printed.
#[derive(Clone)]
pub struct ApiToken(String);

impl fmt::Debug for ApiToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiToken(<redacted>)")
    }
}

impl ApiToken {
    pub fn parse(text: &str) -> Result<Self, String> {
        let token = text.trim();
        if token.is_empty() || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("not a listmonk API token: empty, or with spaces".to_owned());
        }
        Ok(Self(token.to_owned()))
    }

    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the listmonk API token from {}", path.display()))?;
        Self::parse(&text).map_err(|problem| anyhow!("{}: {problem}", path.display()))
    }
}

/// How long one request to listmonk may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The listmonk instance: a client with its credentials.
#[derive(Debug, Clone)]
pub struct Listmonk {
    http: reqwest::Client,
    url: String,
    api_user: String,
    token: ApiToken,
}

impl Listmonk {
    pub fn new(url: &str, api_user: &str, token: ApiToken) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!("devtab-api/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building the HTTP client")?;
        Ok(Self {
            http,
            url: url.trim_end_matches('/').to_owned(),
            api_user: api_user.to_owned(),
            token,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.url))
            .basic_auth(&self.api_user, Some(&self.token.0))
    }

    /// A request and its answer's status and `data`.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(StatusCode, Value), MailError> {
        let mut request = self.request(method.clone(), path);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|err| {
            MailError::Retry(format!("listmonk {method} {path}: {}", err.without_url()))
        })?;
        let status = response.status();
        let answer: Value = response.json().await.unwrap_or(Value::Null);
        Ok((status, answer.get("data").cloned().unwrap_or(Value::Null)))
    }

    /// The instance's transactional templates, by name: their ids.
    pub async fn tx_templates(&self) -> anyhow::Result<BTreeMap<String, i64>> {
        let (status, data) = self
            .call(reqwest::Method::GET, "/api/templates?no_body=true", None)
            .await?;
        if !status.is_success() {
            bail!("listmonk GET /api/templates answered {status}");
        }
        Ok(data
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t.get("type").and_then(Value::as_str) == Some("tx"))
            .filter_map(|t| Some((t.get("name")?.as_str()?.to_owned(), t.get("id")?.as_i64()?)))
            .collect())
    }

    /// Creates a transactional template, or replaces template `id`: its id.
    pub async fn put_tx_template(
        &self,
        id: Option<i64>,
        name: &str,
        subject: &str,
        body: &str,
    ) -> anyhow::Result<i64> {
        let template = json!({ "name": name, "type": "tx", "subject": subject, "body": body });
        let (method, path) = match id {
            Some(id) => (reqwest::Method::PUT, format!("/api/templates/{id}")),
            None => (reqwest::Method::POST, "/api/templates".to_owned()),
        };
        let (status, data) = self.call(method.clone(), &path, Some(&template)).await?;
        if !status.is_success() {
            bail!("listmonk {method} {path} answered {status}");
        }
        id.or_else(|| data.get("id").and_then(Value::as_i64))
            .ok_or_else(|| anyhow!("listmonk {method} {path}: no id in the answer"))
    }
}

/// listmonk as the [`Mailer`].
#[derive(Debug, Clone)]
pub struct ListmonkMailer {
    client: Listmonk,
    templates: BTreeMap<String, i64>,
    list_id: Option<i64>,
    from: Option<String>,
}

impl ListmonkMailer {
    /// Reads the token from its file and builds the client.
    pub fn connect(settings: &ListmonkSettings) -> anyhow::Result<Self> {
        let token = ApiToken::read(&settings.token_file)?;
        Self::with_token(settings, token)
    }

    pub fn with_token(settings: &ListmonkSettings, token: ApiToken) -> anyhow::Result<Self> {
        Ok(Self {
            client: Listmonk::new(&settings.url, &settings.api_user, token)?,
            templates: settings.templates.clone(),
            list_id: settings.list_id,
            from: settings.from.clone(),
        })
    }

    /// The body of `POST /api/tx` for `mail` with template `template`.
    pub fn tx_body(&self, mail: &Mail, template: i64) -> Value {
        let mut body = json!({
            "template_id": template,
            "data": mail.data,
            "content_type": "html",
        });
        if self.list_id.is_some() {
            body["subscriber_email"] = json!(mail.to);
        } else {
            body["subscriber_mode"] = json!("external");
            body["subscriber_emails"] = json!([mail.to]);
        }
        if let Some(from) = &self.from {
            body["from_email"] = json!(from);
        }
        body
    }

    /// Makes `email` a preconfirmed subscriber of the list; one that exists
    /// is left as it is.
    async fn subscribe(&self, email: &str, list: i64) -> Result<(), MailError> {
        let subscriber = json!({
            "email": email,
            "name": "",
            "status": "enabled",
            "lists": [list],
            "preconfirm_subscriptions": true,
        });
        let (status, _) = self
            .client
            .call(reqwest::Method::POST, "/api/subscribers", Some(&subscriber))
            .await?;
        if status.is_success() || status == StatusCode::CONFLICT {
            Ok(())
        } else {
            Err(classify("POST /api/subscribers", status))
        }
    }
}

/// A failed answer: permanent unless trying again may help.
fn classify(what: &str, status: StatusCode) -> MailError {
    let message = format!("listmonk {what} answered {status}");
    if status.is_client_error()
        && status != StatusCode::TOO_MANY_REQUESTS
        && status != StatusCode::REQUEST_TIMEOUT
    {
        MailError::Permanent(message)
    } else {
        MailError::Retry(message)
    }
}

impl Mailer for ListmonkMailer {
    fn name(&self) -> &'static str {
        "listmonk"
    }

    fn send<'a>(&'a self, mail: &'a Mail) -> MailFuture<'a> {
        Box::pin(async move {
            let key = format!("{}.{}", mail.kind, mail.locale);
            let template = *self.templates.get(&key).ok_or_else(|| {
                MailError::Permanent(format!(
                    "no listmonk template for `{key}` (DEVTAB_MAIL_LISTMONK_TEMPLATES; \
                     `devtab-api admin mail templates --apply` prints them)"
                ))
            })?;
            if let Some(list) = self.list_id {
                self.subscribe(&mail.to, list).await?;
            }
            let body = self.tx_body(mail, template);
            let (status, _) = self
                .client
                .call(reqwest::Method::POST, "/api/tx", Some(&body))
                .await?;
            if status.is_success() {
                Ok(())
            } else {
                Err(classify("POST /api/tx", status))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_kind_dot_locale_equals_id() {
        let map = parse_templates(" welcome.en=11, welcome.ru = 12 ,").unwrap();
        assert_eq!(map.get("welcome.en"), Some(&11));
        assert_eq!(map.get("welcome.ru"), Some(&12));
        for bad in [
            "welcome=1",
            "spam.en=1",
            "welcome.de=1",
            "welcome.en=0",
            "welcome.en=x",
        ] {
            assert!(parse_templates(bad).is_err(), "{bad}");
        }
        let settings = ListmonkSettings {
            url: "https://lm.example.invalid".to_owned(),
            api_user: "devtab".to_owned(),
            token_file: "unused".into(),
            templates: map,
            list_id: None,
            from: None,
        };
        let missing = settings.missing_templates();
        assert_eq!(missing.len(), MailKind::ALL.len() * LOCALES.len() - 2);
        assert!(!missing.contains(&"welcome.en".to_owned()));
    }

    #[test]
    fn a_token_is_one_word_and_never_printed() {
        assert!(ApiToken::parse("  abc123\n").is_ok());
        assert!(ApiToken::parse("").is_err());
        assert!(ApiToken::parse("a b").is_err());
        assert!(!format!("{:?}", ApiToken::parse("hidden").unwrap()).contains("hidden"));
    }

    #[test]
    fn statuses_that_retrying_may_help_are_retried() {
        assert!(matches!(
            classify("x", StatusCode::BAD_REQUEST),
            MailError::Permanent(_)
        ));
        assert!(matches!(
            classify("x", StatusCode::NOT_FOUND),
            MailError::Permanent(_)
        ));
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::BAD_GATEWAY,
        ] {
            assert!(
                matches!(classify("x", status), MailError::Retry(_)),
                "{status}"
            );
        }
    }
}
