use std::sync::Arc;

use tokio::sync::RwLock;
use zbus::interface;

use crate::types::{ClientInfo, ServerStatus};

/// Shared state exposed over D-Bus by the daemon.
#[derive(Debug, Clone)]
pub struct RdpServerState {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Debug)]
struct Inner {
    status: ServerStatus,
    bound_address: String,
    clients: Vec<ClientInfo>,
}

impl RdpServerState {
    /// Create a new server state with the given bind address.
    #[must_use]
    pub fn new(bound_address: String) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                status: ServerStatus::Starting,
                bound_address,
                clients: Vec::new(),
            })),
        }
    }

    /// Mark the server as running.
    pub async fn set_running(&self) {
        self.inner.write().await.status = ServerStatus::Running;
    }

    /// Mark the server as stopped.
    pub async fn set_stopped(&self) {
        self.inner.write().await.status = ServerStatus::Stopped;
    }

    /// Mark the server as errored.
    pub async fn set_error(&self) {
        self.inner.write().await.status = ServerStatus::Error;
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

#[interface(name = "com.system76.CosmicRdpServer")]
impl RdpServerInterface {
    /// Get the current server status.
    async fn get_status(&self) -> u8 {
        self.state.inner.read().await.status as u8
    }

    /// Tell the daemon to re-read its configuration file.
    async fn reload(&self) -> bool {
        self.cmd_tx.send(DaemonCommand::Reload).await.is_ok()
    }

    /// Tell the daemon to shut down gracefully.
    async fn stop(&self) -> bool {
        self.cmd_tx.send(DaemonCommand::Stop).await.is_ok()
    }

    /// Get the list of connected clients.
    async fn get_clients(&self) -> Vec<ClientInfo> {
        self.state.inner.read().await.clients.clone()
    }

    /// Whether the server is currently running.
    #[zbus(property)]
    async fn running(&self) -> bool {
        matches!(
            self.state.inner.read().await.status,
            ServerStatus::Running
        )
    }

    /// Number of active client connections.
    #[zbus(property)]
    async fn active_connections(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation)]
        let count = self.state.inner.read().await.clients.len() as u32;
        count
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
