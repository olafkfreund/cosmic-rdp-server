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
/// (Windows mstsc.exe, `FreeRDP`, Remmina).
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

    let mut rdp_server = server::build_server(bind_addr, tls_acceptor);
    rdp_server.run().await.context("RDP server error")?;

    Ok(())
}
