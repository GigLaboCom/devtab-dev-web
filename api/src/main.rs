//! `devtab-api` binary: `serve`, `admin`, `openapi`, `healthcheck`, `version`.
#![forbid(unsafe_code)]

use std::io::{BufRead as _, Write as _};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use axum::Router;
use clap::{Parser, Subcommand};
use devtab_api::admin::{self, AdminCommand, KeysCommand, MailCommand, StripeCommand};
use devtab_api::clock::SystemClock;
use devtab_api::config::Env;
use devtab_api::licence::signer::{self, KeyList, Signer};
use devtab_api::store::audit;
use devtab_api::{AppState, Config, Store, jobs, telemetry};
use serde_json::json;
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Parser)]
#[command(name = "devtab-api", about = "The site's backend API")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP server: the public and the internal listener.
    Serve {
        /// Sign with a key whose `sign_from` has not come yet: only when the
        /// signing key leaked and the waiting key takes over. Logged and
        /// recorded in `admin_audit`.
        #[arg(long)]
        emergency_key: bool,
    },
    /// Administration: licences, users, installations, plans, signing keys.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Print the internal listener's description, an `OpenAPI` document
    /// (what `script/gen` writes into contract/internal-openapi.json).
    Openapi,
    /// Probe `GET /api/readyz` on the local public listener; exit 0 on 200.
    Healthcheck,
    /// Print the version and build SHA.
    Version,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => {
            println!(
                "devtab-api {} ({})",
                env!("CARGO_PKG_VERSION"),
                option_env!("DEVTAB_BUILD_SHA").unwrap_or("dev")
            );
            ExitCode::SUCCESS
        }
        Command::Openapi => match devtab_api::http::internal::openapi().to_pretty_json() {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        },
        Command::Healthcheck => run(healthcheck()),
        Command::Serve { emergency_key } => run(serve(emergency_key)),
        Command::Admin {
            command: AdminCommand::Keys(command),
        } => match keys(&command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err:#}");
                ExitCode::FAILURE
            }
        },
        Command::Admin {
            command: AdminCommand::Stripe(StripeCommand::Check { file }),
        } => run(async move {
            let config = Config::from_env()?;
            admin::stripe_check(&config, &file, &mut std::io::stdout().lock()).await
        }),
        Command::Admin {
            command: AdminCommand::Mail(MailCommand::Templates { apply }),
        } => run(async move {
            let config = Config::from_env()?;
            admin::mail_templates(&config, apply, &mut std::io::stdout().lock()).await
        }),
        Command::Admin { command } => run(admin_db(command)),
    }
}

/// `admin keys …`: no database, no other configuration.
fn keys(command: &KeysCommand) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    match command {
        KeysCommand::Generate {
            kid,
            test,
            out: path,
            list_out,
        } => admin::keys_generate(
            kid.as_deref(),
            *test,
            path,
            list_out.as_deref(),
            OffsetDateTime::now_utc(),
            &mut out,
        ),
        KeysCommand::Check => {
            let var = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
            let env = Env::parse(var("DEVTAB_ENV").as_deref()).map_err(anyhow::Error::msg)?;
            let file = var("DEVTAB_SIGNING_KEY_FILE").map(PathBuf::from);
            let kid = var("DEVTAB_SIGNING_KID");
            let dev_keys = var("DEVTAB_DEV_SIGNING_KEYS_FILE").map(PathBuf::from);
            admin::keys_check(
                env,
                file.as_deref(),
                kid.as_deref(),
                dev_keys.as_deref(),
                OffsetDateTime::now_utc(),
                &mut out,
            )
        }
    }
}

/// The admin commands that use the database, each in one transaction.
async fn admin_db(command: AdminCommand) -> anyhow::Result<()> {
    let config = Config::from_env()?;
    let store =
        Store::connect_lazy(&config.database_url, 1).context("invalid DEVTAB_DATABASE_URL")?;
    let mut tx = store
        .pool()
        .begin()
        .await
        .context("connecting to the database")?;
    let mut stdout = std::io::stdout().lock();
    let mut ctx = admin::Context {
        actor: admin::actor(),
        now: OffsetDateTime::now_utc(),
        out: &mut stdout,
    };
    let mut confirm = || {
        print!("Apply these changes? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).is_ok()
            && matches!(answer.trim(), "y" | "Y" | "yes")
    };
    let providers = admin::providers(&config)?;
    let providers: Vec<&dyn devtab_api::payments::PaymentProvider> =
        providers.iter().map(AsRef::as_ref).collect();
    admin::execute(&command, &mut tx, &mut ctx, &mut confirm, &providers).await?;
    tx.commit().await.context("committing")?;
    Ok(())
}

/// Stripe with its secrets, checked before `serve` starts: the restricted
/// key of the configured mode and the webhook destination's secret.
fn stripe_payments(
    settings: &devtab_api::payments::stripe::StripeSettings,
) -> anyhow::Result<devtab_api::payments::Payments> {
    use devtab_api::payments::stripe::{StripeProvider, keys::WebhookSecret};
    let stripe = StripeProvider::connect(settings.clone())?;
    let path = settings.webhook_secret_file.as_ref().context(
        "DEVTAB_PAYMENTS_WEBHOOK_SECRET_FILE is required for `serve` with DEVTAB_PAYMENTS=stripe",
    )?;
    let webhook_secret = WebhookSecret::read(path)?;
    tracing::info!(
        mode = %settings.mode,
        managed_payments = settings.managed_payments,
        checkout_ui = ?settings.checkout_ui,
        api_version = %settings.api_version,
        "payments: Stripe"
    );
    Ok(devtab_api::payments::Payments {
        stripe,
        webhook_secret,
    })
}

/// Supabase Auth's admin API with the project's secret key, checked before
/// `serve` starts; none without the key, and then a checkout without an
/// account is refused while signed-in purchases work.
fn accounts(config: &Config) -> anyhow::Result<Option<devtab_api::accounts::Accounts>> {
    let Some(settings) = config.accounts() else {
        if config.guest_checkout {
            tracing::warn!(
                "DEVTAB_GUEST_CHECKOUT=true without DEVTAB_SUPABASE_SECRET_KEY_FILE: a checkout \
                 without an account is refused (guest-checkout-unavailable); signed-in purchases work"
            );
        }
        return Ok(None);
    };
    let accounts = devtab_api::accounts::Accounts::connect(&settings, &config.site_url)?;
    tracing::info!(
        guest_checkout = config.guest_checkout,
        "accounts: Supabase Auth's admin API, for the buyers of guest checkouts"
    );
    Ok(Some(accounts))
}

/// The mail provider, its secret read before `serve` starts; none: `mail`
/// jobs wait in the queue.
fn mailer(config: &Config) -> anyhow::Result<Option<Arc<dyn devtab_api::mail::Mailer>>> {
    use devtab_api::mail::MailSettings;
    let mailer = devtab_api::mail::connect(&config.mail)?;
    match (&config.mail, &mailer) {
        (MailSettings::Listmonk(settings), _) => {
            let missing = settings.missing_templates();
            if !missing.is_empty() {
                tracing::warn!(
                    missing = %missing.join(","),
                    "mail: these kinds have no listmonk template and are refused when sent \
                     (DEVTAB_MAIL_LISTMONK_TEMPLATES)"
                );
            }
        }
        (_, None) => tracing::warn!("DEVTAB_MAIL is not set: mails wait in the queue"),
        _ => {}
    }
    if let Some(mailer) = &mailer {
        tracing::info!(provider = mailer.name(), "mail");
    }
    Ok(mailer)
}

/// The signing key from `DEVTAB_SIGNING_KEY_FILE`, checked against the
/// published list: `serve` does not start with a key it may not sign with.
fn signing_key(config: &Config, emergency: bool) -> anyhow::Result<Signer> {
    let (Some(file), Some(kid)) = (&config.signing_key_file, &config.signing_kid) else {
        anyhow::bail!("DEVTAB_SIGNING_KEY_FILE and DEVTAB_SIGNING_KID are required to serve");
    };
    let signer = Signer::load(kid, file)?;
    let keys = KeyList::trusted(config.env, config.dev_signing_keys_file.as_deref())?;
    signer::check(
        &signer,
        &keys,
        config.env,
        OffsetDateTime::now_utc(),
        emergency,
    )
    .map_err(|problems| {
        anyhow::anyhow!(
            "the signing key cannot be used:\n  - {}",
            problems.join("\n  - ")
        )
    })?;
    Ok(signer)
}

fn run(fut: impl Future<Output = anyhow::Result<()>>) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("failed to start the runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(fut) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(emergency_key: bool) -> anyhow::Result<()> {
    let config = Config::from_env()?;
    telemetry::init(&config);

    let signer = signing_key(&config, emergency_key)?;
    let payments = config.stripe.as_ref().map(stripe_payments).transpose()?;
    if !config.env.is_dev() && config.payments.as_deref() == Some(devtab_api::payments::fake::ID) {
        tracing::warn!(
            "DEVTAB_PAYMENTS=fake outside dev: its prices use the test currency, which is served only \
             in dev, so GET /v1/plans offers no paid plan"
        );
    }
    let store = Store::connect_lazy(&config.database_url, config.db_max_connections)
        .context("invalid DEVTAB_DATABASE_URL")?;
    if emergency_key {
        tracing::warn!(
            kid = signer.kid(),
            "signing with --emergency-key: sign_from not checked"
        );
        let recorded = audit::record(
            store.pool(),
            &admin::actor(),
            "keys.emergency",
            None,
            &json!({ "kid": signer.kid() }),
            OffsetDateTime::now_utc(),
        )
        .await;
        if let Err(err) = recorded {
            tracing::warn!(error = %err, "could not record --emergency-key in admin_audit");
        }
    }
    let clock = Arc::new(SystemClock);
    let mut state = AppState::new(store, clock.clone(), signer, &config);
    if let Some(payments) = payments {
        state = state.with_payments(payments);
    }
    if let Some(accounts) = accounts(&config)? {
        state = state.with_accounts(accounts);
    }
    if let Some(mailer) = mailer(&config)? {
        state = state.with_mailer(mailer);
    }
    if let Some(auth) = config.auth() {
        let verifier = devtab_api::auth::Verifier::remote(auth.jwks_url, auth.issuer, clock)
            .context("building the JWKS client")?;
        state = state.with_auth(verifier);
    } else {
        tracing::warn!(
            "DEVTAB_SUPABASE_URL is not set: the routes for signed-in people answer 503"
        );
    }
    let public = TcpListener::bind(config.public_addr)
        .await
        .with_context(|| format!("binding the public listener to {}", config.public_addr))?;
    let internal = TcpListener::bind(config.internal_addr)
        .await
        .with_context(|| format!("binding the internal listener to {}", config.internal_addr))?;
    tracing::info!(
        public = %config.public_addr,
        internal = %config.internal_addr,
        env = ?config.env,
        kid = state.signer.kid(),
        sign_in = state.auth.is_some(),
        version = env!("CARGO_PKG_VERSION"),
        "api started"
    );

    let (stop_tx, stop_rx) = watch::channel(());
    let scheduler = jobs::scheduler::spawn(state.clone(), stop_rx.clone());
    let worker = jobs::worker::spawn(state.clone(), stop_rx.clone());
    let public_app = devtab_api::public_app(&config, state.clone());
    let internal_app = devtab_api::internal_app(&config, state);
    let mut servers = tokio::spawn(async move {
        tokio::try_join!(
            listen("public", public, public_app, stop_rx.clone()),
            listen("internal", internal, internal_app, stop_rx),
        )
        .map(|_| ())
    });

    tokio::select! {
        result = &mut servers => {
            result.context("server task failed")??;
            return Ok(());
        }
        () = shutdown_signal() => {}
    }

    tracing::info!(
        timeout_s = DRAIN_TIMEOUT.as_secs(),
        "shutting down, draining connections"
    );
    let _ = stop_tx.send(());
    if let Ok(result) = tokio::time::timeout(DRAIN_TIMEOUT, servers).await {
        result.context("server task failed")??;
    } else {
        tracing::warn!("drain timeout reached, dropping open connections");
    }
    let _ = scheduler.await;
    let _ = worker.await;
    tracing::info!("api stopped");
    Ok(())
}

/// Serves `app` on `listener` until `stop` changes, then finishes the
/// requests in flight.
async fn listen(
    name: &'static str,
    listener: TcpListener,
    app: Router,
    mut stop: watch::Receiver<()>,
) -> anyhow::Result<()> {
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = stop.changed().await;
    })
    .await
    .with_context(|| format!("the {name} listener failed"))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

/// A minimal HTTP/1.1 probe, so that a container image needs no curl.
async fn healthcheck() -> anyhow::Result<()> {
    // Only the public port matters here; the rest of the configuration is not required.
    let port = match std::env::var("DEVTAB_PUBLIC_ADDR") {
        Ok(addr) if !addr.trim().is_empty() => addr
            .trim()
            .parse::<std::net::SocketAddr>()
            .context("invalid DEVTAB_PUBLIC_ADDR")?
            .port(),
        _ => Config::default().public_addr.port(),
    };
    let probe = async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(b"GET /api/readyz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut buf = [0_u8; 32];
        let n = stream.read(&mut buf).await?;
        anyhow::Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    };
    let status_line = tokio::time::timeout(HEALTHCHECK_TIMEOUT, probe)
        .await
        .context("healthcheck timed out")?
        .with_context(|| format!("no answer on 127.0.0.1:{port}"))?;
    if status_line.starts_with("HTTP/1.1 200 ") {
        Ok(())
    } else {
        anyhow::bail!(
            "readyz answered: {}",
            status_line.lines().next().unwrap_or_default()
        )
    }
}
