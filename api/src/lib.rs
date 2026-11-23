//! The site's backend API: two listeners, one public and one internal.
#![forbid(unsafe_code)]

pub mod config;
pub mod http;
pub mod state;
pub mod store;
pub mod telemetry;

use axum::Router;
pub use config::Config;
pub use state::AppState;
pub use store::Store;

/// The public listener (`DEVTAB_PUBLIC_ADDR`): routes plus middleware.
pub fn public_app(config: &Config, state: AppState) -> Router {
    http::with_layers(http::public_routes(config, state))
}

/// The internal listener (`DEVTAB_INTERNAL_ADDR`), for the site's own pages
/// only: routes plus middleware.
pub fn internal_app(config: &Config, state: AppState) -> Router {
    http::with_layers(http::internal_routes(config, state))
}
