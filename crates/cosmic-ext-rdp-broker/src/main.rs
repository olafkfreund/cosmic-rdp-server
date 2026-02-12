use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

mod broker;
mod config;
mod dbus;
mod pam_auth;
mod proxy;
mod session;
mod spawner;
mod x224;

/// Multi-user RDP session broker for the COSMIC™ desktop environment.
///
/// Accepts all RDP connections on a single port, extracts the username
/// from the X.224 Connection Request cookie, spawns per-user
/// `cosmic-ext-rdp-server` instances, and proxies traffic to them.
#[derive(Parser, Debug)]
#[command(name = "cosmic-ext-rdp-broker", version, about)]
struct Cli {
    /// Path to the broker configuration file (TOML).
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
    let cfg = config::load(cli.config.as_deref())?;

    tracing::info!(
        bind = %cfg.bind,
        port_range = %format!("{}-{}", cfg.port_range_start, cfg.port_range_end),
        max_sessions = cfg.max_sessions,
        idle_timeout = cfg.idle_timeout_secs,
        "Starting cosmic-ext-rdp-broker"
    );

    // Initialize the session registry.
    let registry = session::SessionRegistry::new(
        cfg.port_range_start,
        cfg.port_range_end,
        cfg.max_sessions,
        cfg.state_file.clone(),
    );

    // Restore sessions from state file (if any).
    if let Err(e) = registry.load_state().await {
        tracing::warn!("Failed to load session state: {e}");
    }

    // Start D-Bus interface on the system bus.
    let _dbus_conn = match dbus::start_broker_dbus(registry.clone()).await {
        Ok(conn) => Some(conn),
        Err(e) => {
            tracing::warn!("Failed to start D-Bus interface: {e}");
            tracing::warn!("Broker will run without D-Bus (session management via CLI only)");
            None
        }
    };

    // Spawn the idle session cleanup task.
    let cleanup_registry = registry.clone();
    let idle_timeout = cfg.idle_timeout_secs;
    tokio::spawn(async move {
        broker::idle_cleanup_task(cleanup_registry, idle_timeout).await;
    });

    // Set up signal handlers for graceful shutdown.
    let registry_for_shutdown = registry.clone();
    let shutdown = async move {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received SIGINT, shutting down");
            }
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM, shutting down");
            }
        }

        // Save state before exiting.
        if let Err(e) = registry_for_shutdown.save_state().await {
            tracing::warn!("Failed to save state on shutdown: {e}");
        }
    };

    // Run the broker and shutdown handler concurrently.
    tokio::select! {
        result = broker::run(&cfg, registry) => {
            result.context("broker main loop error")?;
        }
        () = shutdown => {
            tracing::info!("Broker stopped");
        }
    }

    Ok(())
}
