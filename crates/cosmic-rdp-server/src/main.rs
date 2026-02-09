use std::net::SocketAddr;
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
    /// Address to bind the RDP server to
    #[arg(long, default_value = "0.0.0.0")]
    addr: String,

    /// Port to listen on
    #[arg(long, default_value_t = 3389)]
    port: u16,

    /// Path to TLS certificate file (PEM format).
    /// If not provided, a self-signed certificate will be generated.
    #[arg(long)]
    cert: Option<PathBuf>,

    /// Path to TLS private key file (PEM format).
    /// Required if --cert is provided.
    #[arg(long)]
    key: Option<PathBuf>,

    /// Path to configuration file
    #[arg(long, short)]
    config: Option<PathBuf>,

    /// Use a static blue screen instead of live capture (for testing)
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

    let bind_addr: SocketAddr = format!("{}:{}", cli.addr, cli.port)
        .parse()
        .context("invalid bind address")?;

    // Set up TLS
    let tls_acceptor = match (&cli.cert, &cli.key) {
        (Some(cert), Some(key)) => tls::load_from_files(cert, key)?,
        (None, None) => tls::generate_self_signed()?,
        (Some(_), None) => bail!("--cert requires --key"),
        (None, Some(_)) => bail!("--key requires --cert"),
    };

    tracing::info!(%bind_addr, "Starting cosmic-rdp-server");

    if cli.static_display {
        tracing::info!("Using static blue screen display");
        let mut rdp_server = server::build_server(bind_addr, tls_acceptor);
        rdp_server.run().await.context("RDP server error")?;
        return Ok(());
    }

    // Try to start live screen capture via ScreenCast portal + PipeWire
    match rdp_capture::start_capture(None, 4).await {
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
                    // Fall back to static display mode since we can't inject input
                    // but still show the live desktop (view-only)
                    let mut rdp_server =
                        server::build_view_only_server(bind_addr, tls_acceptor, live_display);
                    let _capture = capture_handle;
                    rdp_server.run().await.context("RDP server error")?;
                    return Ok(());
                }
            };

            let mut rdp_server =
                server::build_live_server(bind_addr, tls_acceptor, live_display, input_handler);

            // Keep capture handle alive for the duration of the server
            let _capture = capture_handle;
            rdp_server.run().await.context("RDP server error")?;
        }
        Err(e) => {
            tracing::warn!("Failed to start screen capture: {e:#}");
            tracing::info!("Falling back to static blue screen display");

            let mut rdp_server = server::build_server(bind_addr, tls_acceptor);
            rdp_server.run().await.context("RDP server error")?;
        }
    }

    Ok(())
}
