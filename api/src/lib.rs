//! The site's backend API: two listeners, one public and one internal, the
//! licence core and the plans' catalogue behind the extension's endpoints,
//! the signed-in people of the site's pages, and the admin commands.
#![forbid(unsafe_code)]

pub mod accounts;
pub mod admin;
pub mod auth;
pub mod clock;
pub mod config;
pub mod http;
pub mod jobs;
pub mod licence;
pub mod mail;
pub mod payments;
pub mod plans;
pub mod state;
pub mod store;
pub mod telemetry;
pub mod trial;

use axum::Router;
pub use config::Config;
pub use state::AppState;
pub use store::Store;

/// The public listener (`DEVTAB_PUBLIC_ADDR`): routes plus middleware.
pub fn public_app(config: &Config, state: AppState) -> Router {
    http::with_layers(http::public_routes(config, state.clone())).merge(http::with_layers_limited(
        http::webhooks::routes(state),
        http::webhooks::BODY_LIMIT,
    ))
}

/// The internal listener (`DEVTAB_INTERNAL_ADDR`), for the site's own pages
/// only: routes plus middleware.
pub fn internal_app(config: &Config, state: AppState) -> Router {
    http::with_layers(http::internal_routes(config, state))
}
