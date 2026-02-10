use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;

mod clipboard;
mod config;
mod dbus;
mod server;
mod sound;
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
    let mut cfg = load_and_merge_config(&cli)?;

    // Security check: refuse to bind to non-localhost without authentication
    if !cfg.auth.enable && !is_localhost(cfg.bind.ip()) {
        bail!(
            "auth.enable must be true when binding to non-localhost address {}. \
             Set auth.enable=true with credentials, or bind to 127.0.0.1/::1 for local-only access.",
            cfg.bind.ip()
        );
    }

    // Start D-Bus server for IPC with the settings UI.
    let dbus_state = rdp_dbus::server::RdpServerState::new(cfg.bind.to_string());
    let (_dbus_conn, mut dbus_cmd_rx) =
        dbus::start_dbus_server(dbus_state.clone()).await?;

    loop {
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

        let make_sound = || -> Option<Box<dyn ironrdp_server::SoundServerFactory>> {
            if cfg.audio.enable {
                tracing::info!(
                    channels = cfg.audio.channels,
                    sample_rate = cfg.audio.sample_rate,
                    "Audio forwarding enabled (RDPSND)"
                );
                Some(Box::new(sound::PipeWireAudioFactory::new(
                    cfg.audio.channels,
                    cfg.audio.sample_rate,
                )))
            } else {
                None
            }
        };

        tracing::info!(bind = %cfg.bind, "Starting cosmic-rdp-server");
        dbus_state.set_status(rdp_dbus::types::ServerStatus::Running).await;

        let result = if cfg.static_display {
            tracing::info!("Using static blue screen display");
            let rdp_server = server::build_server(
                cfg.bind, &tls_ctx, auth.as_ref(), make_cliprdr(), make_sound(),
            );
            run_with_shutdown(rdp_server, &mut dbus_cmd_rx).await
        } else {
            run_live_or_fallback(
                &cfg, &tls_ctx, auth.as_ref(), &make_cliprdr, &make_sound, &mut dbus_cmd_rx,
            )
            .await
        };

        match result {
            Ok(ShutdownReason::Reload) => {
                tracing::info!("Reloading configuration");
                cfg = load_and_merge_config(&cli)?;
            }
            Ok(ShutdownReason::Stop | ShutdownReason::Signal) => {
                dbus_state.set_status(rdp_dbus::types::ServerStatus::Stopped).await;
                tracing::info!("Server stopped");
                return Ok(());
            }
            Err(e) => {
                dbus_state.set_status(rdp_dbus::types::ServerStatus::Error).await;
                return Err(e);
            }
        }
    }
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
        _ => tls::generate_self_signed(cfg.bind.ip()),
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
    if cfg.auth.password.is_empty() {
        bail!("auth.enable is true but auth.password is empty");
    }
    tracing::info!(username = %cfg.auth.username, "NLA authentication enabled");
    Ok(Some(server::AuthCredentials {
        username: cfg.auth.username.clone(),
        password: cfg.auth.password.clone(),
        domain: cfg.auth.domain.clone(),
    }))
}

/// Reason the server shut down.
enum ShutdownReason {
    /// Unix signal received.
    Signal,
    /// D-Bus `Reload` command.
    Reload,
    /// D-Bus `Stop` command.
    Stop,
}

/// Try live capture, fall back to static blue screen on failure.
async fn run_live_or_fallback(
    cfg: &config::ServerConfig,
    tls_ctx: &tls::TlsContext,
    auth: Option<&server::AuthCredentials>,
    make_cliprdr: &dyn Fn() -> Option<Box<dyn ironrdp_server::CliprdrServerFactory>>,
    make_sound: &dyn Fn() -> Option<Box<dyn ironrdp_server::SoundServerFactory>>,
    dbus_cmd_rx: &mut tokio::sync::mpsc::Receiver<rdp_dbus::server::DaemonCommand>,
) -> Result<ShutdownReason> {
    match rdp_capture::start_capture(None, cfg.capture.channel_capacity).await {
        Ok((capture_handle, event_rx, desktop_info)) => {
            tracing::info!(
                width = desktop_info.width,
                height = desktop_info.height,
                node_id = desktop_info.node_id,
                "Live screen capture active"
            );

            let live_display = server::LiveDisplay::new(event_rx, &desktop_info);

            let input_handler = match rdp_input::EiInput::new().await {
                Ok(ei_input) => {
                    tracing::info!("Input injection active (libei)");
                    server::LiveInputHandler::new(ei_input)
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize input injection: {e}");
                    tracing::warn!("Input events will be logged but not injected");
                    let rdp_server = server::build_view_only_server(
                        cfg.bind, tls_ctx, auth, live_display, make_cliprdr(), make_sound(),
                    );
                    let _capture = capture_handle;
                    return run_with_shutdown(rdp_server, dbus_cmd_rx).await;
                }
            };

            let rdp_server = server::build_live_server(
                cfg.bind, tls_ctx, auth, live_display, input_handler, make_cliprdr(), make_sound(),
            );
            let _capture = capture_handle;
            run_with_shutdown(rdp_server, dbus_cmd_rx).await
        }
        Err(e) => {
            tracing::warn!("Failed to start screen capture: {e:#}");
            tracing::info!("Falling back to static blue screen display");
            let rdp_server =
                server::build_server(cfg.bind, tls_ctx, auth, make_cliprdr(), make_sound());
            run_with_shutdown(rdp_server, dbus_cmd_rx).await
        }
    }
}

/// Run the RDP server with graceful shutdown on `SIGINT` / `SIGTERM` or
/// D-Bus commands.
async fn run_with_shutdown(
    mut server: ironrdp_server::RdpServer,
    dbus_cmd_rx: &mut tokio::sync::mpsc::Receiver<rdp_dbus::server::DaemonCommand>,
) -> Result<ShutdownReason> {
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("failed to register SIGTERM handler")?;

    tokio::select! {
        result = server.run() => {
            result.context("RDP server error")?;
            Ok(ShutdownReason::Stop)
        }
        result = tokio::signal::ctrl_c() => {
            result.context("failed to listen for SIGINT")?;
            tracing::info!("Received SIGINT, shutting down");
            Ok(ShutdownReason::Signal)
        }
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, shutting down");
            Ok(ShutdownReason::Signal)
        }
        cmd = dbus_cmd_rx.recv() => {
            match cmd {
                Some(rdp_dbus::server::DaemonCommand::Reload) => {
                    tracing::info!("D-Bus: reload requested");
                    Ok(ShutdownReason::Reload)
                }
                Some(rdp_dbus::server::DaemonCommand::Stop) | None => {
                    tracing::info!("D-Bus: stop requested");
                    Ok(ShutdownReason::Stop)
                }
            }
        }
    }
}

/// Returns `true` if the address is a loopback address (`127.0.0.1`, `::1`).
fn is_localhost(ip: std::net::IpAddr) -> bool {
    ip.is_loopback()
}
