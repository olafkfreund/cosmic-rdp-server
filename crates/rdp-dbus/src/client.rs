use crate::types::ClientInfo;

/// D-Bus proxy for the COSMIC RDP Server daemon.
///
/// Used by the settings UI to query status and send commands.
#[zbus::proxy(
    interface = "com.system76.CosmicRdpServer",
    default_service = "com.system76.CosmicRdpServer",
    default_path = "/com/system76/CosmicRdpServer"
)]
pub trait RdpServer {
    /// Get the current server status (see [`ServerStatus`] repr).
    fn get_status(&self) -> zbus::Result<u8>;

    /// Tell the daemon to re-read its configuration file.
    fn reload(&self) -> zbus::Result<bool>;

    /// Tell the daemon to shut down gracefully.
    fn stop(&self) -> zbus::Result<bool>;

    /// Get the list of connected clients.
    fn get_clients(&self) -> zbus::Result<Vec<ClientInfo>>;

    /// Whether the server is currently running.
    #[zbus(property)]
    fn running(&self) -> zbus::Result<bool>;

    /// Number of active client connections.
    #[zbus(property)]
    fn active_connections(&self) -> zbus::Result<u32>;

    /// The address the server is bound to.
    #[zbus(property)]
    fn bound_address(&self) -> zbus::Result<String>;

    /// Emitted when the server status changes.
    #[zbus(signal)]
    fn status_changed(&self, status: u8) -> zbus::Result<()>;

    /// Emitted when a new client connects.
    #[zbus(signal)]
    fn client_connected(&self, address: &str) -> zbus::Result<()>;

    /// Emitted when a client disconnects.
    #[zbus(signal)]
    fn client_disconnected(&self, address: &str) -> zbus::Result<()>;
}
