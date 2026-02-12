use std::sync::Arc;

use tokio::sync::RwLock;
use zbus::interface;
use zbus::message::Header;

use crate::types::ServerStatus;

/// Shared state exposed over D-Bus by the daemon.
#[derive(Debug, Clone)]
pub struct RdpServerState {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Debug)]
struct Inner {
    status: ServerStatus,
    bound_address: String,
}

impl RdpServerState {
    /// Create a new server state with the given bind address.
    #[must_use]
    pub fn new(bound_address: String) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                status: ServerStatus::Starting,
                bound_address,
            })),
        }
    }

    /// Update the server status.
    pub async fn set_status(&self, status: ServerStatus) {
        self.inner.write().await.status = status;
    }

    /// Get current status.
    pub async fn status(&self) -> ServerStatus {
        self.inner.read().await.status
    }
}

/// D-Bus interface implementation for the COSMIC RDP Server.
///
/// Exposes methods, properties, and signals for monitoring and
/// controlling the server.
pub struct RdpServerInterface {
    state: RdpServerState,
    /// Channel to send daemon commands (reload, stop).
    cmd_tx: tokio::sync::mpsc::Sender<DaemonCommand>,
}

/// Commands that can be sent from D-Bus to the daemon.
#[derive(Debug)]
pub enum DaemonCommand {
    /// Re-read the configuration file.
    Reload,
    /// Gracefully shut down the server.
    Stop,
}

impl RdpServerInterface {
    /// Create a new D-Bus interface.
    #[must_use]
    pub fn new(state: RdpServerState, cmd_tx: tokio::sync::mpsc::Sender<DaemonCommand>) -> Self {
        Self { state, cmd_tx }
    }
}

#[interface(name = "io.github.olafkfreund.CosmicExtRdpServer")]
impl RdpServerInterface {
    /// Get the current server status.
    async fn get_status(&self) -> u8 {
        self.state.inner.read().await.status as u8
    }

    /// Tell the daemon to re-read its configuration file.
    ///
    /// Only callers running as the same Unix user may invoke this method.
    async fn reload(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<bool> {
        verify_same_uid(&header, connection).await?;
        Ok(self.cmd_tx.send(DaemonCommand::Reload).await.is_ok())
    }

    /// Tell the daemon to shut down gracefully.
    ///
    /// Only callers running as the same Unix user may invoke this method.
    async fn stop(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<bool> {
        verify_same_uid(&header, connection).await?;
        Ok(self.cmd_tx.send(DaemonCommand::Stop).await.is_ok())
    }

    /// Whether the server is currently running.
    #[zbus(property)]
    async fn running(&self) -> bool {
        matches!(
            self.state.inner.read().await.status,
            ServerStatus::Running
        )
    }

    /// The address the server is bound to.
    #[zbus(property)]
    async fn bound_address(&self) -> String {
        self.state.inner.read().await.bound_address.clone()
    }

    /// Emitted when the server status changes.
    #[zbus(signal)]
    pub async fn status_changed(
        signal_ctxt: &zbus::object_server::SignalEmitter<'_>,
        status: u8,
    ) -> zbus::Result<()>;

    /// Emitted when a new client connects.
    #[zbus(signal)]
    pub async fn client_connected(
        signal_ctxt: &zbus::object_server::SignalEmitter<'_>,
        address: &str,
    ) -> zbus::Result<()>;

    /// Emitted when a client disconnects.
    #[zbus(signal)]
    pub async fn client_disconnected(
        signal_ctxt: &zbus::object_server::SignalEmitter<'_>,
        address: &str,
    ) -> zbus::Result<()>;
}

/// Verify the D-Bus caller is running as the same Unix user as this process.
async fn verify_same_uid(
    header: &Header<'_>,
    connection: &zbus::Connection,
) -> zbus::fdo::Result<()> {
    let sender = header
        .sender()
        .ok_or_else(|| zbus::fdo::Error::AccessDenied("no sender in D-Bus message".into()))?;

    let dbus_proxy = zbus::fdo::DBusProxy::new(connection)
        .await
        .map_err(|e| zbus::fdo::Error::Failed(format!("D-Bus proxy error: {e}")))?;

    let caller_uid = dbus_proxy
        .get_connection_unix_user(zbus::names::BusName::from(sender.to_owned()))
        .await
        .map_err(|e| zbus::fdo::Error::Failed(format!("failed to get caller UID: {e}")))?;

    let my_uid = rustix::process::getuid().as_raw();

    if caller_uid != my_uid {
        return Err(zbus::fdo::Error::AccessDenied(format!(
            "caller UID {caller_uid} does not match server UID {my_uid}"
        )));
    }

    Ok(())
}
