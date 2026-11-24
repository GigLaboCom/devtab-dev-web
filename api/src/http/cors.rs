//! CORS of the licence endpoints. An allowed `Origin` (`DEVTAB_EXTENSION_ORIGINS`)
//! gets `Access-Control-Allow-Origin` with itself (never `*`), `POST`,
//! `Content-Type`, a max age of a day and `Vary: Origin`; credentials are
//! never allowed. Another origin, or none, gets only `Vary: Origin` and its
//! request is handled as usual: the key, the secret and the rate limits
//! protect the endpoints, not this header. `OPTIONS` answers 204 without a
//! body.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_MAX_AGE, ORIGIN, VARY,
};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};

/// The origins allowed to read the licence endpoints' answers.
#[derive(Debug, Clone, Default)]
pub struct AllowedOrigins(Arc<Vec<HeaderValue>>);

impl AllowedOrigins {
    pub fn new(origins: &[String]) -> Self {
        Self(Arc::new(
            origins
                .iter()
                .filter_map(|o| HeaderValue::from_str(o).ok())
                .collect(),
        ))
    }

    fn allows(&self, origin: &HeaderValue) -> bool {
        self.0.iter().any(|o| o == origin)
    }
}

fn apply(response: &mut Response, allowed: Option<HeaderValue>) {
    let headers = response.headers_mut();
    headers.append(VARY, HeaderValue::from_static("Origin"));
    if let Some(origin) = allowed {
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("POST"),
        );
        headers.insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Content-Type"),
        );
        headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("86400"));
    }
}

pub async fn licence_cors(
    State(origins): State<AllowedOrigins>,
    request: Request,
    next: Next,
) -> Response {
    let allowed = request
        .headers()
        .get(ORIGIN)
        .filter(|origin| origins.allows(origin))
        .cloned();
    let mut response = if request.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    apply(&mut response, allowed);
    response
}
