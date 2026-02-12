use anyhow::{Context, Result};
use rdp_dbus::constants::{OBJECT_PATH, SERVICE_NAME};
use rdp_dbus::server::{DaemonCommand, RdpServerInterface, RdpServerState};
use tokio::sync::mpsc;

/// Start the D-Bus server and return a command receiver for daemon control.
///
/// The D-Bus connection runs on the session bus and exposes the
/// `io.github.olafkfreund.CosmicExtRdpServer` interface at
/// `/io/github/olafkfreund/CosmicExtRdpServer`.
///
/// # Errors
///
/// Returns an error if the D-Bus connection cannot be established or
/// the service name is already taken.
pub async fn start_dbus_server(
    state: RdpServerState,
) -> Result<(zbus::Connection, mpsc::Receiver<DaemonCommand>)> {
    let (cmd_tx, cmd_rx) = mpsc::channel(16);

    let iface = RdpServerInterface::new(state, cmd_tx);

    let connection = zbus::connection::Builder::session()
        .context("failed to connect to session D-Bus")?
        .name(SERVICE_NAME)
        .context("failed to request D-Bus service name")?
        .serve_at(OBJECT_PATH, iface)
        .context("failed to serve D-Bus interface")?
        .build()
        .await
        .context("failed to build D-Bus connection")?;

    tracing::info!(service = SERVICE_NAME, "D-Bus server started");

    Ok((connection, cmd_rx))
}
