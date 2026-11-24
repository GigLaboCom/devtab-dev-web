//! Typed configuration from `DEVTAB_*` environment variables, read once at
//! startup; every problem is reported at once.

use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;

use time::OffsetDateTime;
use tracing_subscriber::EnvFilter;

use crate::licence::LicenceSettings;
use crate::licence::token::Spans;

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

    /// `DEVTAB_ENV`'s value; unset is `dev`.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            None | Some("dev") => Ok(Self::Dev),
            Some("staging") => Ok(Self::Staging),
            Some("prod") => Ok(Self::Prod),
            Some(other) => Err(format!(
                "DEVTAB_ENV must be `dev`, `staging` or `prod`, got `{other}`"
            )),
        }
    }
}

/// Requests per hour allowed by each rate limit of the licence endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimits {
    pub activate_per_ip: NonZeroU32,
    pub activate_per_key: NonZeroU32,
    pub refresh_per_ip: NonZeroU32,
    pub refresh_per_installation: NonZeroU32,
    pub deactivate_per_installation: NonZeroU32,
}

impl Default for RateLimits {
    /// The limits the extension's contract proposes.
    fn default() -> Self {
        let n = |v: u32| NonZeroU32::new(v).unwrap_or(NonZeroU32::MIN);
        Self {
            activate_per_ip: n(30),
            activate_per_key: n(10),
            refresh_per_ip: n(60),
            refresh_per_installation: n(6),
            deactivate_per_installation: n(10),
        }
    }
}

/// Runtime configuration.
#[derive(Clone)]
pub struct Config {
    pub env: Env,
    /// Health checks and the extension's licence endpoints.
    pub public_addr: SocketAddr,
    /// Routes for the site's own pages; never published by the proxy.
    pub internal_addr: SocketAddr,
    pub database_url: String,
    pub db_max_connections: u32,
    /// A `tracing` filter, such as `info` or `devtab_api=debug,info`.
    pub log: String,
    /// The only `Host` the licence endpoints answer on; any host when unset.
    pub licence_host: Option<String>,
    /// The `Origin`s allowed to read the licence endpoints' answers.
    pub extension_origins: Vec<String>,
    /// Until then the licence endpoints answer 503 with `Retry-After`.
    pub maintenance_until: Option<OffsetDateTime>,
    /// The private signing key (PKCS#8 PEM) and its id; `serve` needs both.
    pub signing_key_file: Option<PathBuf>,
    pub signing_kid: Option<String>,
    /// `dev` only: the developer's own test public keys (a key list file).
    pub dev_signing_keys_file: Option<PathBuf>,
    pub licence: LicenceSettings,
    pub rate_limits: RateLimits,
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
            .field("licence_host", &self.licence_host)
            .field("extension_origins", &self.extension_origins)
            .field("maintenance_until", &self.maintenance_until)
            .field("signing_key_file", &self.signing_key_file)
            .field("signing_kid", &self.signing_kid)
            .field("dev_signing_keys_file", &self.dev_signing_keys_file)
            .field("licence", &self.licence)
            .field("rate_limits", &self.rate_limits)
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
            database_url: "postgres://devtab_api@127.0.0.1:54330/postgres".to_owned(),
            db_max_connections: 10,
            log: "info".to_owned(),
            licence_host: None,
            extension_origins: Vec::new(),
            maintenance_until: None,
            signing_key_file: None,
            signing_kid: None,
            dev_signing_keys_file: None,
            licence: LicenceSettings::default(),
            rate_limits: RateLimits::default(),
        }
    }
}

/// An extension's origin as Chrome sends it: `chrome-extension://` and a
/// 32-letter id of `a`–`p`.
fn is_extension_origin(origin: &str) -> bool {
    origin
        .strip_prefix("chrome-extension://")
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b)))
}

fn is_host_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
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

        match Env::parse(get("DEVTAB_ENV").as_deref()) {
            Ok(env) => config.env = env,
            Err(problem) => problems.push(problem),
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

        config.read_licence(&get, &mut problems);

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

    /// The licence endpoints' settings: host, origins, maintenance, the
    /// signing key, the token's days, seats and rate limits.
    /// The signing key's file and id, and in `dev` the developer's own key list.
    fn read_signing(&mut self, get: &impl Fn(&str) -> Option<String>, problems: &mut Vec<String>) {
        let config = self;
        config.signing_key_file = get("DEVTAB_SIGNING_KEY_FILE").map(PathBuf::from);
        config.signing_kid = get("DEVTAB_SIGNING_KID");
        if config.signing_key_file.is_some() != config.signing_kid.is_some() {
            problems.push(
                "DEVTAB_SIGNING_KEY_FILE and DEVTAB_SIGNING_KID go together: set both".to_owned(),
            );
        }
        config.dev_signing_keys_file = get("DEVTAB_DEV_SIGNING_KEYS_FILE").map(PathBuf::from);
        if config.dev_signing_keys_file.is_some() && !config.env.is_dev() {
            problems.push(
                "DEVTAB_DEV_SIGNING_KEYS_FILE is accepted only in dev: staging and prod trust \
                 only config/signing-keys.json"
                    .to_owned(),
            );
        }
    }

    fn read_licence(&mut self, get: &impl Fn(&str) -> Option<String>, problems: &mut Vec<String>) {
        let config = self;
        match get("DEVTAB_LICENCE_HOST") {
            Some(host) if is_host_name(&host) => config.licence_host = Some(host.to_ascii_lowercase()),
            Some(host) => problems.push(format!(
                "DEVTAB_LICENCE_HOST must be a host name such as licence.example.invalid, got `{host}`"
            )),
            None if !config.env.is_dev() => {
                problems.push("DEVTAB_LICENCE_HOST is required in staging and prod".to_owned());
            }
            None => {}
        }

        match get("DEVTAB_EXTENSION_ORIGINS") {
            Some(list) => {
                for origin in list.split(',').map(str::trim).filter(|o| !o.is_empty()) {
                    if is_extension_origin(origin) {
                        config.extension_origins.push(origin.to_owned());
                    } else {
                        problems.push(format!(
                            "DEVTAB_EXTENSION_ORIGINS takes chrome-extension://<32 letters a-p>, \
                             comma-separated; got `{origin}`"
                        ));
                    }
                }
            }
            None if !config.env.is_dev() => {
                problems
                    .push("DEVTAB_EXTENSION_ORIGINS is required in staging and prod".to_owned());
            }
            None => {}
        }

        if let Some(until) = get("DEVTAB_MAINTENANCE_UNTIL") {
            match crate::clock::parse_rfc3339(&until) {
                Ok(at) => config.maintenance_until = Some(at),
                Err(_) => problems.push(format!(
                    "DEVTAB_MAINTENANCE_UNTIL must be an RFC 3339 time such as \
                     2026-12-01T06:00:00Z, got `{until}`"
                )),
            }
        }

        config.read_signing(get, problems);

        let mut number = |key: &str, default: u32, min: u32, max: u32| -> u32 {
            match get(key) {
                None => default,
                Some(text) => match text.parse::<u32>() {
                    Ok(n) if (min..=max).contains(&n) => n,
                    _ => {
                        problems.push(format!(
                            "{key} must be a whole number from {min} to {max}, got `{text}`"
                        ));
                        default
                    }
                },
            }
        };
        let check_days = number("DEVTAB_TOKEN_CHECK_DAYS", 7, 1, 365);
        let window_days = number("DEVTAB_TOKEN_WINDOW_DAYS", 30, 1, 365);
        let warn_days = number("DEVTAB_TOKEN_WARN_DAYS", 7, 0, 365);
        let dunning_days = number("DEVTAB_TOKEN_DUNNING_DAYS", 14, 0, 365);
        let release_after_days = number("DEVTAB_RELEASE_AFTER_DAYS", 60, 1, 3650);
        let activation_churn = number("DEVTAB_ACTIVATION_CHURN", 3, 1, 100);
        let defaults = RateLimits::default();
        let mut quota = |key: &str, default: NonZeroU32| {
            NonZeroU32::new(number(key, default.get(), 1, 1_000_000)).unwrap_or(default)
        };
        config.rate_limits = RateLimits {
            activate_per_ip: quota("DEVTAB_RATE_ACTIVATE_PER_IP", defaults.activate_per_ip),
            activate_per_key: quota("DEVTAB_RATE_ACTIVATE_PER_KEY", defaults.activate_per_key),
            refresh_per_ip: quota("DEVTAB_RATE_REFRESH_PER_IP", defaults.refresh_per_ip),
            refresh_per_installation: quota(
                "DEVTAB_RATE_REFRESH_PER_INSTALLATION",
                defaults.refresh_per_installation,
            ),
            deactivate_per_installation: quota(
                "DEVTAB_RATE_DEACTIVATE_PER_INSTALLATION",
                defaults.deactivate_per_installation,
            ),
        };
        // An active token needs iat < chk ≤ warn ≤ exp.
        if check_days > window_days || warn_days > window_days - check_days.min(window_days) {
            problems.push(format!(
                "the token's days must keep check ≤ window − warn: DEVTAB_TOKEN_CHECK_DAYS \
                 {check_days}, DEVTAB_TOKEN_WINDOW_DAYS {window_days}, DEVTAB_TOKEN_WARN_DAYS {warn_days}"
            ));
        }
        config.licence = LicenceSettings {
            spans: Spans::from_days(check_days, window_days, warn_days, dunning_days),
            release_after_days,
            activation_churn,
        };
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

    const ORIGIN: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";

    #[test]
    fn the_internal_listener_leaves_loopback_outside_dev() {
        for env in ["staging", "prod"] {
            let config = load(&[
                ("DEVTAB_ENV", env),
                ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
                ("DEVTAB_LICENCE_HOST", "licence.example.invalid"),
                ("DEVTAB_EXTENSION_ORIGINS", ORIGIN),
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
            ("DEVTAB_LICENCE_HOST", "Licence.Example.Invalid"),
            ("DEVTAB_EXTENSION_ORIGINS", ORIGIN),
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
    fn the_licence_settings_default_to_the_contract() {
        let config = load(&[("DEVTAB_DATABASE_URL", "postgres://u@h/db")]).unwrap();
        assert_eq!(config.licence, LicenceSettings::default());
        assert_eq!(config.licence.spans, Spans::from_days(7, 30, 7, 14));
        assert_eq!(config.licence.release_after_days, 60);
        assert_eq!(config.licence.activation_churn, 3);
        assert_eq!(config.rate_limits, RateLimits::default());
        assert_eq!(config.rate_limits.activate_per_ip.get(), 30);
        assert_eq!(config.rate_limits.activate_per_key.get(), 10);
        assert_eq!(config.rate_limits.refresh_per_ip.get(), 60);
        assert_eq!(config.rate_limits.refresh_per_installation.get(), 6);
        assert_eq!(config.rate_limits.deactivate_per_installation.get(), 10);
        assert_eq!(config.licence_host, None);
        assert!(config.extension_origins.is_empty());
        assert_eq!(config.maintenance_until, None);
        assert_eq!((config.signing_key_file, config.signing_kid), (None, None));
    }

    #[test]
    fn reads_the_licence_settings() {
        let config = load(&[
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_LICENCE_HOST", "licence.example.invalid"),
            (
                "DEVTAB_EXTENSION_ORIGINS",
                &format!("{ORIGIN}, chrome-extension://pppppppppppppppppppppppppppppppp"),
            ),
            ("DEVTAB_MAINTENANCE_UNTIL", "2026-12-01T06:00:00Z"),
            ("DEVTAB_SIGNING_KEY_FILE", "/run/secrets/signing"),
            ("DEVTAB_SIGNING_KID", "p1"),
            ("DEVTAB_TOKEN_CHECK_DAYS", "5"),
            ("DEVTAB_TOKEN_WINDOW_DAYS", "20"),
            ("DEVTAB_TOKEN_WARN_DAYS", "4"),
            ("DEVTAB_TOKEN_DUNNING_DAYS", "10"),
            ("DEVTAB_RELEASE_AFTER_DAYS", "90"),
            ("DEVTAB_ACTIVATION_CHURN", "2"),
            ("DEVTAB_RATE_ACTIVATE_PER_IP", "3"),
            ("DEVTAB_RATE_ACTIVATE_PER_KEY", "2"),
            ("DEVTAB_RATE_REFRESH_PER_IP", "9"),
            ("DEVTAB_RATE_REFRESH_PER_INSTALLATION", "1"),
            ("DEVTAB_RATE_DEACTIVATE_PER_INSTALLATION", "4"),
        ])
        .unwrap();
        assert_eq!(
            config.licence_host.as_deref(),
            Some("licence.example.invalid")
        );
        assert_eq!(config.extension_origins.len(), 2);
        assert_eq!(
            config.maintenance_until,
            OffsetDateTime::from_unix_timestamp(1_796_104_800).ok()
        );
        assert_eq!(
            config.signing_key_file,
            Some(PathBuf::from("/run/secrets/signing"))
        );
        assert_eq!(config.signing_kid.as_deref(), Some("p1"));
        assert_eq!(config.licence.spans, Spans::from_days(5, 20, 4, 10));
        assert_eq!(config.licence.release_after_days, 90);
        assert_eq!(config.licence.activation_churn, 2);
        assert_eq!(config.rate_limits.activate_per_ip.get(), 3);
        assert_eq!(config.rate_limits.activate_per_key.get(), 2);
        assert_eq!(config.rate_limits.refresh_per_ip.get(), 9);
        assert_eq!(config.rate_limits.refresh_per_installation.get(), 1);
        assert_eq!(config.rate_limits.deactivate_per_installation.get(), 4);
    }

    #[test]
    fn staging_and_prod_need_the_licence_host_and_the_extension_origins() {
        for env in ["staging", "prod"] {
            let err = load(&[
                ("DEVTAB_ENV", env),
                ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ])
            .unwrap_err();
            assert_eq!(err.0.len(), 2, "{env}: {err}");
        }
    }

    #[test]
    fn reports_every_licence_problem_at_once() {
        let err = load(&[
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_LICENCE_HOST", "https://licence.example.invalid/"),
            ("DEVTAB_EXTENSION_ORIGINS", "*, chrome-extension://short"),
            ("DEVTAB_MAINTENANCE_UNTIL", "tomorrow"),
            ("DEVTAB_SIGNING_KID", "p1"),
            ("DEVTAB_TOKEN_CHECK_DAYS", "0"),
            ("DEVTAB_RELEASE_AFTER_DAYS", "-1"),
            ("DEVTAB_RATE_ACTIVATE_PER_IP", "0"),
        ])
        .unwrap_err();
        // host, two origins, maintenance, the key pair, three numbers.
        assert_eq!(err.0.len(), 8, "{err}");
    }

    #[test]
    fn the_dev_key_list_is_read_in_dev_and_refused_elsewhere() {
        let config = load(&[
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            (
                "DEVTAB_DEV_SIGNING_KEYS_FILE",
                ".state/signing/test-keys.json",
            ),
        ])
        .unwrap();
        assert_eq!(
            config.dev_signing_keys_file,
            Some(PathBuf::from(".state/signing/test-keys.json"))
        );
        for env in ["staging", "prod"] {
            let err = load(&[
                ("DEVTAB_ENV", env),
                ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
                ("DEVTAB_LICENCE_HOST", "licence.example.invalid"),
                ("DEVTAB_EXTENSION_ORIGINS", ORIGIN),
                ("DEVTAB_DEV_SIGNING_KEYS_FILE", "/tmp/keys.json"),
            ])
            .unwrap_err();
            assert_eq!(err.0.len(), 1, "{env}: {err}");
            assert!(err.0[0].contains("only in dev"));
        }
    }

    #[test]
    fn the_token_days_must_keep_their_order() {
        let err = load(&[
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_TOKEN_CHECK_DAYS", "10"),
            ("DEVTAB_TOKEN_WINDOW_DAYS", "14"),
            ("DEVTAB_TOKEN_WARN_DAYS", "7"),
        ])
        .unwrap_err();
        assert_eq!(err.0.len(), 1, "{err}");
    }

    #[test]
    fn the_database_url_is_required() {
        let err = load(&[("DEVTAB_DATABASE_URL", "  ")]).unwrap_err();
        assert_eq!(err.0, vec!["DEVTAB_DATABASE_URL is required".to_owned()]);
    }
}
