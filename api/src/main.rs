//! `devtab-api` binary: `serve`, `healthcheck`, `version`.
#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context as _;
use axum::Router;
use clap::{Parser, Subcommand};
use devtab_api::{AppState, Config, Store, telemetry};
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
    Serve,
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
        Command::Healthcheck => run(healthcheck()),
        Command::Serve => run(serve()),
    }
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

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    telemetry::init(&config);

    let store = Store::connect_lazy(&config.database_url, config.db_max_connections)
        .context("invalid DEVTAB_DATABASE_URL")?;
    let state = AppState::new(store);
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
        version = env!("CARGO_PKG_VERSION"),
        "api started"
    );

    let (stop_tx, stop_rx) = watch::channel(());
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
    axum::serve(listener, app)
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
