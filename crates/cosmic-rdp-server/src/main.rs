use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;

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

    // Load config file, then override with CLI args
    let mut cfg = config::load_config(cli.config.as_deref())?;

    // CLI overrides take precedence over config file
    if let Some(addr) = &cli.addr {
        let port = cli.port.unwrap_or(cfg.bind.port());
        cfg.bind = format!("{addr}:{port}")
            .parse()
            .context("invalid bind address")?;
    } else if let Some(port) = cli.port {
        cfg.bind.set_port(port);
    }

    if let Some(cert) = cli.cert {
        cfg.cert_path = Some(cert);
    }
    if let Some(key) = cli.key {
        cfg.key_path = Some(key);
    }
    if cli.static_display {
        cfg.static_display = true;
    }

    // Validate cert/key pairing
    match (&cfg.cert_path, &cfg.key_path) {
        (Some(_), None) => bail!("--cert requires --key (or set key_path in config)"),
        (None, Some(_)) => bail!("--key requires --cert (or set cert_path in config)"),
        _ => {}
    }

    // Set up TLS
    let tls_ctx = match (&cfg.cert_path, &cfg.key_path) {
        (Some(cert), Some(key)) => tls::load_from_files(cert, key)?,
        _ => tls::generate_self_signed()?,
    };

    // Set up auth credentials
    let auth = if cfg.auth.enable {
        if cfg.auth.username.is_empty() {
            bail!("auth.enable is true but auth.username is empty");
        }
        tracing::info!(username = %cfg.auth.username, "NLA authentication enabled");
        Some(server::AuthCredentials {
            username: cfg.auth.username.clone(),
            password: cfg.auth.password.clone(),
            domain: cfg.auth.domain.clone(),
        })
    } else {
        None
    };

    tracing::info!(bind = %cfg.bind, "Starting cosmic-rdp-server");

    if cfg.static_display {
        tracing::info!("Using static blue screen display");
        let mut rdp_server = server::build_server(cfg.bind, &tls_ctx, auth.as_ref());
        rdp_server.run().await.context("RDP server error")?;
        return Ok(());
    }

    // Try to start live screen capture via ScreenCast portal + PipeWire
    match rdp_capture::start_capture(None, cfg.capture.channel_capacity).await {
        Ok((capture_handle, frame_rx, desktop_info)) => {
            tracing::info!(
                width = desktop_info.width,
                height = desktop_info.height,
                node_id = desktop_info.node_id,
                "Live screen capture active"
            );

            let live_display = server::LiveDisplay::new(frame_rx, &desktop_info);

            // Try to set up input injection via libei
            let input_handler = match rdp_input::EnigoInput::new() {
                Ok(enigo) => {
                    tracing::info!("Input injection active (libei)");
                    server::LiveInputHandler::new(enigo)
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize input injection: {e}");
                    tracing::warn!("Input events will be logged but not injected");
                    let mut rdp_server = server::build_view_only_server(
                        cfg.bind,
                        &tls_ctx,
                        auth.as_ref(),
                        live_display,
                    );
                    let _capture = capture_handle;
                    rdp_server.run().await.context("RDP server error")?;
                    return Ok(());
                }
            };

            let mut rdp_server = server::build_live_server(
                cfg.bind,
                &tls_ctx,
                auth.as_ref(),
                live_display,
                input_handler,
            );

            // Keep capture handle alive for the duration of the server
            let _capture = capture_handle;
            rdp_server.run().await.context("RDP server error")?;
        }
        Err(e) => {
            tracing::warn!("Failed to start screen capture: {e:#}");
            tracing::info!("Falling back to static blue screen display");

            let mut rdp_server = server::build_server(cfg.bind, &tls_ctx, auth.as_ref());
            rdp_server.run().await.context("RDP server error")?;
        }
    }

    Ok(())
}
