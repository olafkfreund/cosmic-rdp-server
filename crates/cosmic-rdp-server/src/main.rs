use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;

mod clipboard;
mod config;
mod server;
mod tls;

/// RDP server for the COSMIC Desktop Environment.
///
/// Allows remote access to COSMIC desktops using standard RDP clients
/// (Windows `mstsc.exe`, `FreeRDP`, Remmina).
#[derive(Parser, Debug)]
#[command(name = "cosmic-rdp-server", version, about)]
struct Cli {
    /// Address to bind the RDP server to.
    #[arg(long)]
    addr: Option<String>,

    /// Port to listen on.
    #[arg(long)]
    port: Option<u16>,

    /// Path to TLS certificate file (PEM format).
    /// If not provided, a self-signed certificate will be generated.
    #[arg(long)]
    cert: Option<PathBuf>,

    /// Path to TLS private key file (PEM format).
    /// Required if --cert is provided.
    #[arg(long)]
    key: Option<PathBuf>,

    /// Path to configuration file (TOML).
    #[arg(long, short)]
    config: Option<PathBuf>,

    /// Use a static blue screen instead of live capture (for testing).
    #[arg(long)]
    static_display: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let cfg = load_and_merge_config(&cli)?;

    let tls_ctx = setup_tls(&cfg)?;
    let auth = setup_auth(&cfg)?;

    let make_cliprdr = || -> Option<Box<dyn ironrdp_server::CliprdrServerFactory>> {
        if cfg.clipboard.enable {
            tracing::info!("Clipboard sharing enabled");
            Some(Box::new(clipboard::LocalClipboardFactory::new()))
        } else {
            None
        }
    };

    tracing::info!(bind = %cfg.bind, "Starting cosmic-rdp-server");

    if cfg.static_display {
        tracing::info!("Using static blue screen display");
        let rdp_server =
            server::build_server(cfg.bind, &tls_ctx, auth.as_ref(), make_cliprdr());
        return run_with_shutdown(rdp_server).await;
    }

    run_live_or_fallback(&cfg, &tls_ctx, auth.as_ref(), &make_cliprdr).await
}

/// Load config from file and apply CLI overrides.
fn load_and_merge_config(cli: &Cli) -> Result<config::ServerConfig> {
    let mut cfg = config::load_config(cli.config.as_deref())?;

    if let Some(addr) = &cli.addr {
        let port = cli.port.unwrap_or(cfg.bind.port());
        cfg.bind = format!("{addr}:{port}")
            .parse()
            .context("invalid bind address")?;
    } else if let Some(port) = cli.port {
        cfg.bind.set_port(port);
    }

    if let Some(cert) = &cli.cert {
        cfg.cert_path = Some(cert.clone());
    }
    if let Some(key) = &cli.key {
        cfg.key_path = Some(key.clone());
    }
    if cli.static_display {
        cfg.static_display = true;
    }

    match (&cfg.cert_path, &cfg.key_path) {
        (Some(_), None) => bail!("--cert requires --key (or set key_path in config)"),
        (None, Some(_)) => bail!("--key requires --cert (or set cert_path in config)"),
        _ => {}
    }

    Ok(cfg)
}

/// Initialise TLS from files or generate self-signed.
fn setup_tls(cfg: &config::ServerConfig) -> Result<tls::TlsContext> {
    match (&cfg.cert_path, &cfg.key_path) {
        (Some(cert), Some(key)) => tls::load_from_files(cert, key),
        _ => tls::generate_self_signed(),
    }
}

/// Build auth credentials if NLA is enabled.
fn setup_auth(cfg: &config::ServerConfig) -> Result<Option<server::AuthCredentials>> {
    if !cfg.auth.enable {
        return Ok(None);
    }
    if cfg.auth.username.is_empty() {
        bail!("auth.enable is true but auth.username is empty");
    }
    tracing::info!(username = %cfg.auth.username, "NLA authentication enabled");
    Ok(Some(server::AuthCredentials {
        username: cfg.auth.username.clone(),
        password: cfg.auth.password.clone(),
        domain: cfg.auth.domain.clone(),
    }))
}

/// Try live capture, fall back to static blue screen on failure.
async fn run_live_or_fallback(
    cfg: &config::ServerConfig,
    tls_ctx: &tls::TlsContext,
    auth: Option<&server::AuthCredentials>,
    make_cliprdr: &dyn Fn() -> Option<Box<dyn ironrdp_server::CliprdrServerFactory>>,
) -> Result<()> {
    match rdp_capture::start_capture(None, cfg.capture.channel_capacity).await {
        Ok((capture_handle, frame_rx, desktop_info)) => {
            tracing::info!(
                width = desktop_info.width,
                height = desktop_info.height,
                node_id = desktop_info.node_id,
                "Live screen capture active"
            );

            let live_display = server::LiveDisplay::new(frame_rx, &desktop_info);

            let input_handler = match rdp_input::EnigoInput::new() {
                Ok(enigo) => {
                    tracing::info!("Input injection active (libei)");
                    server::LiveInputHandler::new(enigo)
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize input injection: {e}");
                    tracing::warn!("Input events will be logged but not injected");
                    let rdp_server = server::build_view_only_server(
                        cfg.bind, tls_ctx, auth, live_display, make_cliprdr(),
                    );
                    let _capture = capture_handle;
                    return run_with_shutdown(rdp_server).await;
                }
            };

            let rdp_server = server::build_live_server(
                cfg.bind, tls_ctx, auth, live_display, input_handler, make_cliprdr(),
            );
            let _capture = capture_handle;
            run_with_shutdown(rdp_server).await
        }
        Err(e) => {
            tracing::warn!("Failed to start screen capture: {e:#}");
            tracing::info!("Falling back to static blue screen display");
            let rdp_server = server::build_server(cfg.bind, tls_ctx, auth, make_cliprdr());
            run_with_shutdown(rdp_server).await
        }
    }
}

/// Run the RDP server with graceful shutdown on `SIGINT` / `SIGTERM`.
async fn run_with_shutdown(mut server: ironrdp_server::RdpServer) -> Result<()> {
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("failed to register SIGTERM handler")?;

    tokio::select! {
        result = server.run() => {
            result.context("RDP server error")?;
        }
        result = tokio::signal::ctrl_c() => {
            result.context("failed to listen for SIGINT")?;
            tracing::info!("Received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, shutting down");
        }
    }

    tracing::info!("Server stopped");
    Ok(())
}
