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
use crate::payments;
use crate::payments::stripe::{self, CheckoutUi, StripeMode, StripeSettings};
use crate::plans::catalogue::CatalogueSettings;

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
    /// `GET /v1/plans`: requests a minute, by client address.
    pub plans_per_ip: NonZeroU32,
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
            plans_per_ip: n(60),
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
    /// The site's address, `https://<host>`: the plans' checkout and manage
    /// addresses.
    pub site_url: String,
    /// The payment provider whose prices the plans' endpoints serve
    /// (`fake` in development); none: no paid plan is offered.
    pub payments: Option<String>,
    /// Stripe's settings, when `DEVTAB_PAYMENTS=stripe`.
    pub stripe: Option<StripeSettings>,
    /// `maxAge` of `GET /v1/plans`, seconds.
    pub plans_max_age_s: u32,
    /// The Supabase project's address (`https://<project>.supabase.co`;
    /// locally the API of `supabase start`): its JWKS checks the access tokens
    /// of signed-in people. Unset (development only): a route that needs a
    /// signed-in person answers 503.
    pub supabase_url: Option<String>,
    /// The `iss` the access tokens carry; `<supabase_url>/auth/v1` unless set.
    pub jwt_issuer: Option<String>,
}

/// Where the access tokens of signed-in people are checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSettings {
    /// `<supabase_url>/auth/v1/.well-known/jwks.json`.
    pub jwks_url: String,
    pub issuer: String,
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
            .field("site_url", &self.site_url)
            .field("payments", &self.payments)
            .field("stripe", &self.stripe)
            .field("plans_max_age_s", &self.plans_max_age_s)
            .field("supabase_url", &self.supabase_url)
            .field("jwt_issuer", &self.jwt_issuer)
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
            site_url: DEV_SITE_URL.to_owned(),
            payments: Some(payments::fake::ID.to_owned()),
            stripe: None,
            plans_max_age_s: 86_400,
            supabase_url: None,
            jwt_issuer: None,
        }
    }
}

/// The site's address in development: the host the extension's development
/// configuration accepts for checkout and manage addresses.
pub const DEV_SITE_URL: &str = "https://web.example.invalid";

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

/// `https://<host>`, with at most one slash after it, as the site's address;
/// the host name is lower-cased and the slash dropped.
fn site_url(text: &str) -> Option<String> {
    let host = text.strip_prefix("https://")?;
    let host = host.strip_suffix('/').unwrap_or(host);
    (is_host_name(host) && host.len() <= 200)
        .then(|| format!("https://{}", host.to_ascii_lowercase()))
}

/// `http(s)://<host>[:<port>]`, with at most one slash after it, as the
/// Supabase project's address: the slash dropped. `http` only in `dev` (the
/// local stack).
fn supabase_url(text: &str, env: Env) -> Option<String> {
    let (scheme, rest) = text.split_once("://")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (rest, None),
    };
    let scheme_ok = scheme == "https" || (scheme == "http" && env.is_dev());
    let port_ok = port.is_none_or(|p| p.parse::<u16>().is_ok_and(|p| p > 0));
    (scheme_ok && port_ok && is_host_name(host) && host.len() <= 200)
        .then(|| format!("{scheme}://{}", rest.to_ascii_lowercase()))
}

/// `true` or `false`; `default` when unset.
fn flag(
    get: &impl Fn(&str) -> Option<String>,
    key: &str,
    default: bool,
    problems: &mut Vec<String>,
) -> bool {
    match get(key).as_deref() {
        None => default,
        Some("true") => true,
        Some("false") => false,
        Some(other) => {
            problems.push(format!("{key} must be `true` or `false`, got `{other}`"));
            default
        }
    }
}

/// A whole number from `min` to `max`; `default` when unset.
fn whole(
    get: &impl Fn(&str) -> Option<String>,
    key: &str,
    (default, min, max): (u32, u32, u32),
    problems: &mut Vec<String>,
) -> u32 {
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
}

/// Stripe's API address: `https://<host>[:port]`, no path; plain `http` only
/// to a stand-in on the loopback address, and only in dev.
fn stripe_api_url(text: &str, env: Env) -> Option<String> {
    let text = text.strip_suffix('/').unwrap_or(text);
    let (scheme, rest) = text.split_once("://")?;
    let host = rest.rsplit_once(':').map_or(rest, |(host, port)| {
        if port.parse::<u16>().is_ok() {
            host
        } else {
            rest
        }
    });
    let loopback = matches!(host, "127.0.0.1" | "localhost" | "[::1]");
    let ok = match scheme {
        "https" => is_host_name(host),
        "http" => env.is_dev() && loopback,
        _ => false,
    };
    ok.then(|| text.to_owned())
}

/// Stripe's settings (`DEVTAB_PAYMENTS=stripe`); the secrets stay in their
/// files, which `serve` and the commands read and check.
fn read_stripe(
    env: Env,
    get: &impl Fn(&str) -> Option<String>,
    problems: &mut Vec<String>,
) -> Option<StripeSettings> {
    let before = problems.len();
    let mode = read_stripe_mode(env, get, problems);
    let api_key_file = get("DEVTAB_PAYMENTS_API_KEY_FILE").map(PathBuf::from);
    if api_key_file.is_none() {
        problems.push(
            "DEVTAB_PAYMENTS_API_KEY_FILE is required with DEVTAB_PAYMENTS=stripe: the file of a \
             restricted key (rk_…)"
                .to_owned(),
        );
    }
    let webhook_secret_file = get("DEVTAB_PAYMENTS_WEBHOOK_SECRET_FILE").map(PathBuf::from);
    let managed_payments = flag(get, "DEVTAB_STRIPE_MANAGED_PAYMENTS", true, problems);
    let automatic_tax = flag(get, "DEVTAB_STRIPE_AUTOMATIC_TAX", false, problems);
    if managed_payments && automatic_tax {
        problems.push(
            "DEVTAB_STRIPE_AUTOMATIC_TAX=true needs DEVTAB_STRIPE_MANAGED_PAYMENTS=false: with \
             Managed Payments Stripe handles the tax itself"
                .to_owned(),
        );
    }
    let checkout_ui = match get("DEVTAB_STRIPE_CHECKOUT_UI") {
        None => CheckoutUi::Hosted,
        Some(text) => CheckoutUi::parse(&text).unwrap_or_else(|| {
            problems.push(format!(
                "DEVTAB_STRIPE_CHECKOUT_UI must be `hosted` or `embedded`, got `{text}`"
            ));
            CheckoutUi::Hosted
        }),
    };
    let publishable_key = get("DEVTAB_STRIPE_PUBLISHABLE_KEY");
    check_publishable_key(publishable_key.as_deref(), mode, checkout_ui, problems);
    let (api_url, api_version) = read_stripe_api(env, get, problems);
    let timeout_s = whole(get, "DEVTAB_STRIPE_TIMEOUT", (8, 1, 60), problems);
    let webhook_tolerance_s = whole(
        get,
        "DEVTAB_STRIPE_WEBHOOK_TOLERANCE",
        (300, 60, 600),
        problems,
    );
    match (mode, api_key_file) {
        (Some(mode), Some(api_key_file)) if problems.len() == before => Some(StripeSettings {
            mode,
            api_key_file,
            webhook_secret_file,
            managed_payments,
            automatic_tax,
            checkout_ui,
            publishable_key,
            api_url,
            api_version,
            timeout_s,
            webhook_tolerance_s,
        }),
        _ => None,
    }
}

/// `DEVTAB_PAYMENTS_ENV`: required, and never a sandbox in prod.
fn read_stripe_mode(
    env: Env,
    get: &impl Fn(&str) -> Option<String>,
    problems: &mut Vec<String>,
) -> Option<StripeMode> {
    let Some(text) = get("DEVTAB_PAYMENTS_ENV") else {
        problems.push("DEVTAB_PAYMENTS_ENV is required with DEVTAB_PAYMENTS=stripe".to_owned());
        return None;
    };
    match StripeMode::parse(&text) {
        Some(StripeMode::Sandbox) if env == Env::Prod => {
            problems.push(
                "DEVTAB_PAYMENTS_ENV=sandbox is refused in prod: prod takes real payments"
                    .to_owned(),
            );
            None
        }
        Some(mode) => Some(mode),
        None => {
            problems.push(format!(
                "DEVTAB_PAYMENTS_ENV must be `sandbox` or `live`, got `{text}`"
            ));
            None
        }
    }
}

/// The publishable key: required by the embedded form, and of the mode.
fn check_publishable_key(
    key: Option<&str>,
    mode: Option<StripeMode>,
    checkout_ui: CheckoutUi,
    problems: &mut Vec<String>,
) {
    match (key, mode) {
        (None, _) if checkout_ui == CheckoutUi::Embedded => problems.push(
            "DEVTAB_STRIPE_PUBLISHABLE_KEY is required with DEVTAB_STRIPE_CHECKOUT_UI=embedded"
                .to_owned(),
        ),
        (Some(key), Some(mode)) => {
            let prefix = if mode.is_live() {
                "pk_live_"
            } else {
                "pk_test_"
            };
            let valid = key.strip_prefix(prefix).is_some_and(|rest| {
                !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            });
            if !valid {
                problems.push(format!(
                    "DEVTAB_STRIPE_PUBLISHABLE_KEY must be a publishable key of the {mode} mode, \
                     {prefix}…"
                ));
            }
        }
        _ => {}
    }
}

/// `DEVTAB_STRIPE_API_URL` and `DEVTAB_STRIPE_API_VERSION`, or their defaults.
fn read_stripe_api(
    env: Env,
    get: &impl Fn(&str) -> Option<String>,
    problems: &mut Vec<String>,
) -> (String, String) {
    let api_url = match get("DEVTAB_STRIPE_API_URL") {
        None => stripe::DEFAULT_API_URL.to_owned(),
        Some(text) => stripe_api_url(&text, env).unwrap_or_else(|| {
            problems.push(format!(
                "DEVTAB_STRIPE_API_URL must be https://<host>[:port] (http only to the loopback \
                 address in dev), got `{text}`"
            ));
            String::new()
        }),
    };
    let api_version = match get("DEVTAB_STRIPE_API_VERSION") {
        None => stripe::DEFAULT_API_VERSION.to_owned(),
        Some(text) if stripe::is_supported_api_version(&text) => text,
        Some(text) => {
            problems.push(format!(
                "DEVTAB_STRIPE_API_VERSION must be a Stripe API version such as {} and not older \
                 than {}, got `{text}`",
                stripe::DEFAULT_API_VERSION,
                stripe::OLDEST_API_VERSION
            ));
            String::new()
        }
    };
    (api_url, api_version)
}

impl Config {
    /// Where access tokens are checked, when sign-in is configured.
    pub fn auth(&self) -> Option<AuthSettings> {
        let url = self.supabase_url.as_ref()?;
        Some(AuthSettings {
            jwks_url: format!("{url}/auth/v1/.well-known/jwks.json"),
            issuer: self
                .jwt_issuer
                .clone()
                .unwrap_or_else(|| format!("{url}/auth/v1")),
        })
    }

    /// What the plans' catalogue is built with.
    pub fn catalogue(&self) -> CatalogueSettings {
        CatalogueSettings {
            site_url: self.site_url.clone(),
            provider: self.payments.clone(),
            max_age_s: self.plans_max_age_s,
            test_currencies: self.env.is_dev(),
        }
    }

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
        config.read_plans(&get, &mut problems);
        config.read_auth(&get, &mut problems);

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

    /// The plans' settings: the site's address, the payment provider and the
    /// catalogue's `maxAge`.
    fn read_plans(&mut self, get: &impl Fn(&str) -> Option<String>, problems: &mut Vec<String>) {
        let config = self;
        match get("DEVTAB_SITE_URL") {
            Some(text) => match site_url(&text) {
                Some(url) => config.site_url = url,
                None => problems.push(format!(
                    "DEVTAB_SITE_URL must be https:// and a host name, no port or path, such as \
                     https://web.example.invalid; got `{text}`"
                )),
            },
            None if !config.env.is_dev() => {
                problems.push("DEVTAB_SITE_URL is required in staging and prod".to_owned());
            }
            None => {}
        }

        config.payments = match get("DEVTAB_PAYMENTS") {
            Some(name) if payments::KNOWN.contains(&name.as_str()) => Some(name),
            Some(name) => {
                problems.push(format!(
                    "DEVTAB_PAYMENTS must be one of {}, got `{name}`",
                    payments::KNOWN.join(", ")
                ));
                None
            }
            // Development has the fake provider; elsewhere no provider is assumed.
            None if config.env.is_dev() => Some(payments::fake::ID.to_owned()),
            None => None,
        };
        if config.env == Env::Prod && config.payments.as_deref() == Some(payments::fake::ID) {
            problems.push(
                "DEVTAB_PAYMENTS=fake is refused in prod: the fake provider's prices use a test \
                 currency"
                    .to_owned(),
            );
        }

        if config.payments.as_deref() == Some(stripe::ID) {
            config.stripe = read_stripe(config.env, get, problems);
        }

        if let Some(text) = get("DEVTAB_PLANS_MAX_AGE") {
            match text.parse::<u32>() {
                Ok(n) if (3_600..=604_800).contains(&n) => config.plans_max_age_s = n,
                _ => problems.push(format!(
                    "DEVTAB_PLANS_MAX_AGE must be seconds from 3600 to 604800 (what the extension \
                     keeps), got `{text}`"
                )),
            }
        }
    }

    /// The Supabase project, whose JWKS and issuer check the access tokens.
    fn read_auth(&mut self, get: &impl Fn(&str) -> Option<String>, problems: &mut Vec<String>) {
        let config = self;
        match get("DEVTAB_SUPABASE_URL") {
            Some(text) => match supabase_url(&text, config.env) {
                Some(url) => config.supabase_url = Some(url),
                None => problems.push(format!(
                    "DEVTAB_SUPABASE_URL must be https:// and a host name (http:// too in dev), an \
                     optional port and no path, such as https://project.supabase.co; got `{text}`"
                )),
            },
            None if !config.env.is_dev() => {
                problems.push("DEVTAB_SUPABASE_URL is required in staging and prod".to_owned());
            }
            None => {}
        }
        if let Some(issuer) = get("DEVTAB_JWT_ISSUER") {
            if issuer.starts_with("https://")
                || (issuer.starts_with("http://") && config.env.is_dev())
            {
                config.jwt_issuer = Some(issuer.trim_end_matches('/').to_owned());
            } else {
                problems.push(format!(
                    "DEVTAB_JWT_ISSUER must be an https:// address (http:// too in dev), got `{issuer}`"
                ));
            }
        }
    }

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

    /// The licence endpoints' settings: host, origins, maintenance, the
    /// signing key, the token's days, seats and rate limits.
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
            plans_per_ip: quota("DEVTAB_RATE_PLANS_PER_IP", defaults.plans_per_ip),
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
    const SUPABASE: &str = "https://project.example.invalid";

    #[test]
    fn the_internal_listener_leaves_loopback_outside_dev() {
        for env in ["staging", "prod"] {
            let config = load(&[
                ("DEVTAB_ENV", env),
                ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
                ("DEVTAB_LICENCE_HOST", "licence.example.invalid"),
                ("DEVTAB_EXTENSION_ORIGINS", ORIGIN),
                ("DEVTAB_SITE_URL", "https://web.example.invalid"),
                ("DEVTAB_SUPABASE_URL", SUPABASE),
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
            ("DEVTAB_SITE_URL", "https://Web.Example.Invalid/"),
            ("DEVTAB_SUPABASE_URL", SUPABASE),
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
        assert_eq!(config.site_url, "https://web.example.invalid");
        assert_eq!(config.payments, None, "no provider is assumed outside dev");
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
    fn staging_and_prod_need_the_licence_host_the_extension_origins_the_site_and_supabase() {
        for env in ["staging", "prod"] {
            let err = load(&[
                ("DEVTAB_ENV", env),
                ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ])
            .unwrap_err();
            assert_eq!(err.0.len(), 4, "{env}: {err}");
        }
    }

    #[test]
    fn the_plans_settings_default_to_development() {
        let config = load(&[("DEVTAB_DATABASE_URL", "postgres://u@h/db")]).unwrap();
        assert_eq!(config.site_url, "https://web.example.invalid");
        assert_eq!(config.payments.as_deref(), Some("fake"));
        assert_eq!(config.plans_max_age_s, 86_400);
        assert_eq!(config.rate_limits.plans_per_ip.get(), 60);
        let catalogue = config.catalogue();
        assert!(catalogue.test_currencies);
        assert_eq!(catalogue.provider.as_deref(), Some("fake"));
    }

    #[test]
    fn reads_the_plans_settings() {
        let config = load(&[
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_SITE_URL", "https://site.example.invalid"),
            ("DEVTAB_PAYMENTS", "fake"),
            ("DEVTAB_PLANS_MAX_AGE", "3600"),
            ("DEVTAB_RATE_PLANS_PER_IP", "5"),
        ])
        .unwrap();
        assert_eq!(config.site_url, "https://site.example.invalid");
        assert_eq!(config.plans_max_age_s, 3_600);
        assert_eq!(config.rate_limits.plans_per_ip.get(), 5);
        let err = load(&[
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_SITE_URL", "http://site.example.invalid:4321/pro"),
            ("DEVTAB_PAYMENTS", "paypal"),
            ("DEVTAB_PLANS_MAX_AGE", "60"),
        ])
        .unwrap_err();
        assert_eq!(err.0.len(), 3, "{err}");
    }

    #[test]
    fn the_fake_provider_is_refused_in_prod_and_its_currency_served_only_in_dev() {
        let outside = |env: &'static str| {
            [
                ("DEVTAB_ENV", env),
                ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
                ("DEVTAB_LICENCE_HOST", "licence.example.invalid"),
                ("DEVTAB_EXTENSION_ORIGINS", ORIGIN),
                ("DEVTAB_SITE_URL", "https://web.example.invalid"),
                ("DEVTAB_SUPABASE_URL", SUPABASE),
                ("DEVTAB_PAYMENTS", "fake"),
            ]
        };
        let config = load(&outside("staging")).unwrap();
        assert!(
            !config.catalogue().test_currencies,
            "staging serves no test currency"
        );
        let err = load(&outside("prod")).unwrap_err();
        assert_eq!(err.0.len(), 1, "{err}");
        assert!(err.0[0].contains("refused in prod"));
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
                ("DEVTAB_SITE_URL", "https://web.example.invalid"),
                ("DEVTAB_SUPABASE_URL", SUPABASE),
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

    fn stripe_env<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut pairs = vec![
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_PAYMENTS", "stripe"),
            ("DEVTAB_PAYMENTS_ENV", "sandbox"),
            (
                "DEVTAB_PAYMENTS_API_KEY_FILE",
                "/run/secrets/payments-api-key",
            ),
        ];
        pairs.extend_from_slice(extra);
        pairs
    }

    #[test]
    fn stripe_s_settings_default_to_managed_payments_and_the_hosted_page() {
        let config = load(&stripe_env(&[])).unwrap();
        let stripe = config.stripe.unwrap();
        assert_eq!(stripe.mode, StripeMode::Sandbox);
        assert_eq!(
            stripe.api_key_file,
            PathBuf::from("/run/secrets/payments-api-key")
        );
        assert_eq!(stripe.webhook_secret_file, None);
        assert!(stripe.managed_payments);
        assert!(!stripe.automatic_tax);
        assert_eq!(stripe.checkout_ui, CheckoutUi::Hosted);
        assert_eq!(stripe.publishable_key, None);
        assert_eq!(stripe.api_url, "https://api.stripe.com");
        assert_eq!(stripe.api_version, stripe::DEFAULT_API_VERSION);
        assert_eq!((stripe.timeout_s, stripe.webhook_tolerance_s), (8, 300));
        let fake = load(&[("DEVTAB_DATABASE_URL", "postgres://u@h/db")]).unwrap();
        assert_eq!(fake.stripe, None, "no Stripe settings without the provider");
    }

    #[test]
    fn reads_stripe_s_settings() {
        let config = load(&stripe_env(&[
            (
                "DEVTAB_PAYMENTS_WEBHOOK_SECRET_FILE",
                "/run/secrets/payments-webhook-secret",
            ),
            ("DEVTAB_STRIPE_MANAGED_PAYMENTS", "false"),
            ("DEVTAB_STRIPE_AUTOMATIC_TAX", "true"),
            ("DEVTAB_STRIPE_CHECKOUT_UI", "embedded"),
            ("DEVTAB_STRIPE_PUBLISHABLE_KEY", "pk_test_abc"),
            ("DEVTAB_STRIPE_API_URL", "http://127.0.0.1:12111/"),
            ("DEVTAB_STRIPE_API_VERSION", "2026-09-30.endive"),
            ("DEVTAB_STRIPE_TIMEOUT", "3"),
            ("DEVTAB_STRIPE_WEBHOOK_TOLERANCE", "120"),
        ]))
        .unwrap();
        let stripe = config.stripe.unwrap();
        assert_eq!(
            stripe.webhook_secret_file,
            Some(PathBuf::from("/run/secrets/payments-webhook-secret"))
        );
        assert!(!stripe.managed_payments);
        assert!(stripe.automatic_tax);
        assert_eq!(stripe.checkout_ui, CheckoutUi::Embedded);
        assert_eq!(stripe.publishable_key.as_deref(), Some("pk_test_abc"));
        assert_eq!(stripe.api_url, "http://127.0.0.1:12111");
        assert_eq!(stripe.api_version, "2026-09-30.endive");
        assert_eq!((stripe.timeout_s, stripe.webhook_tolerance_s), (3, 120));
    }

    #[test]
    fn reports_every_stripe_problem_at_once() {
        let err = load(&[
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_PAYMENTS", "stripe"),
            ("DEVTAB_PAYMENTS_ENV", "test"),
            ("DEVTAB_STRIPE_MANAGED_PAYMENTS", "yes"),
            ("DEVTAB_STRIPE_AUTOMATIC_TAX", "true"),
            ("DEVTAB_STRIPE_CHECKOUT_UI", "embedded"),
            ("DEVTAB_STRIPE_API_URL", "http://api.example.invalid"),
            ("DEVTAB_STRIPE_API_VERSION", "2025-03-31.basil"),
            ("DEVTAB_STRIPE_TIMEOUT", "0"),
            ("DEVTAB_STRIPE_WEBHOOK_TOLERANCE", "1"),
        ])
        .unwrap_err();
        // env, key file, managed flag, tax with managed payments, the
        // publishable key, the address, the version, two numbers.
        assert_eq!(err.0.len(), 9, "{err}");
    }

    #[test]
    fn keys_of_the_other_mode_and_sandboxes_in_prod_are_refused() {
        let err = load(&stripe_env(&[(
            "DEVTAB_STRIPE_PUBLISHABLE_KEY",
            "pk_live_abc",
        )]))
        .unwrap_err();
        assert!(err.0[0].contains("pk_test_"), "{err}");
        let err = load(&[
            ("DEVTAB_ENV", "prod"),
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_LICENCE_HOST", "licence.example.invalid"),
            ("DEVTAB_EXTENSION_ORIGINS", ORIGIN),
            ("DEVTAB_SITE_URL", "https://web.example.invalid"),
            ("DEVTAB_SUPABASE_URL", "https://project.example.invalid"),
            ("DEVTAB_PAYMENTS", "stripe"),
            ("DEVTAB_PAYMENTS_ENV", "sandbox"),
            (
                "DEVTAB_PAYMENTS_API_KEY_FILE",
                "/run/secrets/payments-api-key",
            ),
        ])
        .unwrap_err();
        assert_eq!(err.0.len(), 1, "{err}");
        assert!(err.0[0].contains("refused in prod"));
        let staging = load(&stripe_env(&[
            ("DEVTAB_ENV", "staging"),
            ("DEVTAB_LICENCE_HOST", "licence.example.invalid"),
            ("DEVTAB_EXTENSION_ORIGINS", ORIGIN),
            ("DEVTAB_SITE_URL", "https://web.example.invalid"),
            ("DEVTAB_SUPABASE_URL", "https://project.example.invalid"),
            ("DEVTAB_STRIPE_API_URL", "http://127.0.0.1:12111"),
        ]))
        .unwrap_err();
        assert!(staging.0[0].contains("DEVTAB_STRIPE_API_URL"), "{staging}");
    }

    #[test]
    fn the_jwks_and_the_issuer_come_from_the_supabase_url() {
        let config = load(&[("DEVTAB_DATABASE_URL", "postgres://u@h/db")]).unwrap();
        assert_eq!(config.auth(), None, "no sign-in without a project");
        let config = load(&[
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_SUPABASE_URL", "http://127.0.0.1:54332/"),
        ])
        .unwrap();
        assert_eq!(
            config.auth(),
            Some(AuthSettings {
                jwks_url: "http://127.0.0.1:54332/auth/v1/.well-known/jwks.json".to_owned(),
                issuer: "http://127.0.0.1:54332/auth/v1".to_owned(),
            })
        );
        let config = load(&[
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_SUPABASE_URL", "https://Project.Example.Invalid"),
            ("DEVTAB_JWT_ISSUER", "https://auth.example.invalid/auth/v1/"),
        ])
        .unwrap();
        let auth = config.auth().unwrap();
        assert_eq!(
            auth.jwks_url,
            "https://project.example.invalid/auth/v1/.well-known/jwks.json"
        );
        assert_eq!(auth.issuer, "https://auth.example.invalid/auth/v1");
    }

    #[test]
    fn a_supabase_url_with_a_path_or_plain_http_outside_dev_is_refused() {
        let err = load(&[
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            (
                "DEVTAB_SUPABASE_URL",
                "https://project.example.invalid/auth/v1",
            ),
            ("DEVTAB_JWT_ISSUER", "issuer"),
        ])
        .unwrap_err();
        assert_eq!(err.0.len(), 2, "{err}");
        let err = load(&[
            ("DEVTAB_ENV", "staging"),
            ("DEVTAB_DATABASE_URL", "postgres://u@h/db"),
            ("DEVTAB_LICENCE_HOST", "licence.example.invalid"),
            ("DEVTAB_EXTENSION_ORIGINS", ORIGIN),
            ("DEVTAB_SITE_URL", "https://web.example.invalid"),
            ("DEVTAB_SUPABASE_URL", "http://127.0.0.1:54332"),
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
