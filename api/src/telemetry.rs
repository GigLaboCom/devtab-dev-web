//! Logging: JSON lines in staging and prod, compact human-readable lines in dev.

use tracing_subscriber::EnvFilter;

use crate::config::Config;

/// Installs the global tracing subscriber. Call once at startup.
pub fn init(config: &Config) {
    let filter = EnvFilter::try_new(&config.log).unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if config.env.is_dev() {
        builder.compact().init();
    } else {
        builder
            .json()
            .flatten_event(true)
            .with_current_span(false)
            .init();
    }
}
