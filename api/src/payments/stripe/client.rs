//! Stripe's REST API over HTTPS: form-encoded requests, JSON answers, the
//! pinned `Stripe-Version` on every request, an idempotency key on every
//! `POST`, and a few retries where Stripe says they help.
//!
//! TLS is reqwest's rustls with the Mozilla roots compiled in
//! (`rustls-tls-webpki-roots`), so that a container needs no system
//! certificate store; the same client setup as the JWKS fetch.

use std::time::Duration;

use anyhow::Context as _;
use reqwest::{RequestBuilder, Response, StatusCode};
use serde_json::Value;

use super::keys::ApiKey;
use crate::payments::ProviderError;

/// The pauses before each retry of a request that may succeed later.
pub const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];

/// The largest answer read from Stripe.
const MAX_ANSWER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    key: ApiKey,
    version: String,
    retry_delays: Vec<Duration>,
}

impl Client {
    /// A client for `base` (`https://api.stripe.com`, or a stand-in in
    /// development and tests) with API version `version`; each attempt of a
    /// request may take `timeout`.
    pub fn new(base: &str, key: ApiKey, version: &str, timeout: Duration) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("devtab-api/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building the HTTP client")?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            key,
            version: version.to_owned(),
            retry_delays: RETRY_DELAYS.to_vec(),
        })
    }

    /// Other pauses between retries (tests use none).
    #[must_use]
    pub fn with_retry_delays(mut self, delays: Vec<Duration>) -> Self {
        self.retry_delays = delays;
        self
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// `GET <path>?<query>`.
    pub async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, ProviderError> {
        self.send(|| self.request(reqwest::Method::GET, path).query(query))
            .await
    }

    /// `POST <path>` with a form body; `idempotency_key` makes a repeated
    /// request return the first one's answer instead of acting twice.
    pub async fn post(
        &self,
        path: &str,
        form: &[(String, String)],
        idempotency_key: &str,
    ) -> Result<Value, ProviderError> {
        self.send(|| {
            self.request(reqwest::Method::POST, path)
                .header("Idempotency-Key", idempotency_key)
                .form(form)
        })
        .await
    }

    fn request(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.key.expose())
            .header("Stripe-Version", &self.version)
    }

    async fn send(&self, build: impl Fn() -> RequestBuilder) -> Result<Value, ProviderError> {
        let mut attempt = 0;
        loop {
            let (result, retry) = match build().send().await {
                Ok(response) => answer(response).await,
                Err(err) => (
                    Err(ProviderError::Unavailable(format!(
                        "no answer from Stripe: {}",
                        without_url(&err)
                    ))),
                    true,
                ),
            };
            match (&result, self.retry_delays.get(attempt)) {
                (Err(_), Some(delay)) if retry => {
                    tokio::time::sleep(*delay).await;
                    attempt += 1;
                }
                _ => return result,
            }
        }
    }
}

/// An answer and whether its request is worth repeating: on 409 (the same
/// idempotency key still in progress), 429 and 5xx, unless Stripe says not
/// to (`Stripe-Should-Retry: false`); other errors only when it says to.
async fn answer(response: Response) -> (Result<Value, ProviderError>, bool) {
    let status = response.status();
    let should_retry = response
        .headers()
        .get("Stripe-Should-Retry")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "true");
    let body = match read_limited(response).await {
        Ok(body) => body,
        Err(problem) => return (Err(ProviderError::Unavailable(problem)), true),
    };
    if status.is_success() {
        return match serde_json::from_slice(&body) {
            Ok(value) => (Ok(value), false),
            Err(err) => (
                Err(ProviderError::Unavailable(format!(
                    "Stripe answered {status} with a body that is not JSON: {err}"
                ))),
                false,
            ),
        };
    }
    let message = error_message(&body).unwrap_or_else(|| format!("HTTP {status}"));
    let transient = status == StatusCode::CONFLICT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error();
    let error = if status == StatusCode::NOT_FOUND {
        ProviderError::NotFound(message)
    } else if transient {
        ProviderError::Unavailable(message)
    } else {
        ProviderError::Refused(format!("{status}: {message}"))
    };
    (Err(error), should_retry.unwrap_or(transient))
}

async fn read_limited(mut response: Response) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| format!("reading Stripe's answer: {}", without_url(&err)))?
    {
        if body.len() + chunk.len() > MAX_ANSWER_BYTES {
            return Err(format!("Stripe's answer is over {MAX_ANSWER_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// `error.message` of Stripe's error body.
fn error_message(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let error = value.get("error")?;
    let message = error.get("message")?.as_str()?;
    Some(match error.get("code").and_then(Value::as_str) {
        Some(code) => format!("{message} ({code})"),
        None => message.to_owned(),
    })
}

/// A transport error without its URL: the query of a lookup may be long and
/// says nothing the caller's message does not.
fn without_url(err: &reqwest::Error) -> String {
    let mut err = err.to_string();
    if let Some(at) = err.find(" for url (") {
        err.truncate(at);
    }
    err
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stripe_s_error_message_is_read_with_its_code() {
        let body = br#"{"error":{"message":"No such price: 'price_x'","code":"resource_missing","type":"invalid_request_error"}}"#;
        assert_eq!(
            error_message(body).as_deref(),
            Some("No such price: 'price_x' (resource_missing)")
        );
        assert_eq!(error_message(b"<html>"), None);
    }
}
