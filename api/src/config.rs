//! Typed configuration from `DEVTAB_*` environment variables, read once at
//! startup; every problem is reported at once.

use std::fmt;
use std::net::SocketAddr;

use tracing_subscriber::EnvFilter;

/// Deployment environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Env {
    Dev,
    Staging,
    Prod,
}

impl Env {
    pub fn is_dev(self) -> bool {
        self == Self::Dev
    }
}

/// Runtime configuration.
#[derive(Clone)]
pub struct Config {
    pub env: Env,
    /// Health checks now; the extension's and the payment provider's routes later.
    pub public_addr: SocketAddr,
    /// Routes for the site's own pages; never published by the proxy.
    pub internal_addr: SocketAddr,
    pub database_url: String,
    pub db_max_connections: u32,
    /// A `tracing` filter, such as `info` or `devtab_api=debug,info`.
    pub log: String,
}

// The database URL may carry a password, so it stays out of debug output.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("env", &self.env)
            .field("public_addr", &self.public_addr)
            .field("internal_addr", &self.internal_addr)
            .field("database_url", &"<redacted>")
            .field("db_max_connections", &self.db_max_connections)
            .field("log", &self.log)
            .finish()
    }
}

/// Every problem found in the environment.
#[derive(Debug, thiserror::Error)]
pub struct ConfigError(pub Vec<String>);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "invalid configuration:")?;
        for problem in &self.0 {
            writeln!(f, "  - {problem}")?;
        }
        Ok(())
    }
}

impl Default for Config {
    /// Development defaults; the database URL is the local one from
    /// `script/db-up`, which `from_lookup` still requires to be set.
    fn default() -> Self {
        Self {
            env: Env::Dev,
            public_addr: SocketAddr::from(([0, 0, 0, 0], 8080)),
            internal_addr: SocketAddr::from(([127, 0, 0, 1], 8081)),
            database_url: "postgres://devtab_api@127.0.0.1:54330/devtab".to_owned(),
            db_max_connections: 10,
            log: "info".to_owned(),
        }
    }
}

impl Config {
    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads the configuration through `lookup`, so that tests need no real
    /// environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |key: &str| {
            lookup(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let mut problems = Vec::new();
        let mut config = Self::default();

        match get("DEVTAB_ENV").as_deref() {
            None | Some("dev") => config.env = Env::Dev,
            Some("staging") => config.env = Env::Staging,
            Some("prod") => config.env = Env::Prod,
            Some(other) => problems.push(format!(
                "DEVTAB_ENV must be `dev`, `staging` or `prod`, got `{other}`"
            )),
        }

        // Only a development machine keeps the internal listener on loopback by
        // default; in a container it has to be reachable by the site's container.
        if !config.env.is_dev() {
            config.internal_addr = SocketAddr::from(([0, 0, 0, 0], 8081));
        }
        for (key, slot) in [
            ("DEVTAB_PUBLIC_ADDR", &mut config.public_addr),
            ("DEVTAB_INTERNAL_ADDR", &mut config.internal_addr),
        ] {
            if let Some(addr) = get(key) {
                match addr.parse() {
                    Ok(parsed) => *slot = parsed,
                    Err(_) => problems.push(format!(
                        "{key} must be `host:port` with an IP host, got `{addr}`"
                    )),
                }
            }
        }

        match get("DEVTAB_DATABASE_URL") {
            None => problems.push("DEVTAB_DATABASE_URL is required".to_owned()),
            Some(url) if url.starts_with("postgres://") || url.starts_with("postgresql://") => {
                config.database_url = url;
            }
            Some(_) => problems
                .push("DEVTAB_DATABASE_URL must be a postgres:// or postgresql:// URL".to_owned()),
        }

        if let Some(max) = get("DEVTAB_DB_MAX_CONNECTIONS") {
            match max.parse::<u32>() {
                Ok(n) if n > 0 => config.db_max_connections = n,
                _ => problems.push(format!(
                    "DEVTAB_DB_MAX_CONNECTIONS must be a positive integer, got `{max}`"
                )),
            }
        }

        if let Some(filter) = get("DEVTAB_LOG") {
            match EnvFilter::try_new(&filter) {
                Ok(_) => config.log = filter,
                Err(err) => problems.push(format!(
                    "DEVTAB_LOG must be a tracing filter such as `info`, got `{filter}`: {err}"
                )),
            }
        }

        if problems.is_empty() {
            Ok(config)
        } else {
            Err(ConfigError(problems))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn load(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Config::from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults_apply_when_only_the_database_url_is_set() {
        let config = load(&[("DEVTAB_DATABASE_URL", "postgres://u@h/db")]).unwrap();
        assert_eq!(config.env, Env::Dev);
        assert_eq!(config.public_addr, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(config.internal_addr, "127.0.0.1:8081".parse().unwrap());
        assert_eq!(config.database_url, "postgres://u@h/db");
        assert_eq!(config.db_max_connections, 10);
        assert_eq!(config.log, "info");
    }

    #[test]
    fn the_internal_listener_leaves_loopback_outside_dev() {
        for env in ["staging", "prod"] {
            let config = load(&[
                ("DEVTAB_ENV", env),
                ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ])
            .unwrap();
            assert_eq!(
                config.internal_addr,
                "0.0.0.0:8081".parse().unwrap(),
                "{env}"
            );
        }
    }

    #[test]
    fn reads_values() {
        let config = load(&[
            ("DEVTAB_ENV", "prod"),
            ("DEVTAB_PUBLIC_ADDR", "127.0.0.1:9000"),
            ("DEVTAB_INTERNAL_ADDR", " 127.0.0.1:9001 "),
            (
                "DEVTAB_DATABASE_URL",
                "postgresql://devtab_api:hunter2@db:5432/devtab",
            ),
            ("DEVTAB_DB_MAX_CONNECTIONS", "4"),
            ("DEVTAB_LOG", "devtab_api=debug,info"),
        ])
        .unwrap();
        assert_eq!(config.env, Env::Prod);
        assert_eq!(config.public_addr, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(config.internal_addr, "127.0.0.1:9001".parse().unwrap());
        assert_eq!(
            config.database_url,
            "postgresql://devtab_api:hunter2@db:5432/devtab"
        );
        assert_eq!(config.db_max_connections, 4);
        assert_eq!(config.log, "devtab_api=debug,info");
        assert!(!format!("{config:?}").contains("hunter2"));
    }

    #[test]
    fn reports_every_problem_at_once() {
        let err = load(&[
            ("DEVTAB_ENV", "production"),
            ("DEVTAB_PUBLIC_ADDR", "localhost:8080"),
            ("DEVTAB_INTERNAL_ADDR", "nope"),
            ("DEVTAB_DATABASE_URL", "mysql://x"),
            ("DEVTAB_DB_MAX_CONNECTIONS", "0"),
            ("DEVTAB_LOG", "devtab_api=loud"),
        ])
        .unwrap_err();
        assert_eq!(err.0.len(), 6, "{err}");
    }

    #[test]
    fn the_database_url_is_required() {
        let err = load(&[("DEVTAB_DATABASE_URL", "  ")]).unwrap_err();
        assert_eq!(err.0, vec!["DEVTAB_DATABASE_URL is required".to_owned()]);
    }
}
