/// D-Bus proxy for the RDP Server daemon.
///
/// Used by the settings UI to query status and send commands.
#[zbus::proxy(
    interface = "io.github.olafkfreund.CosmicExtRdpServer",
    default_service = "io.github.olafkfreund.CosmicExtRdpServer",
    default_path = "/io/github/olafkfreund/CosmicExtRdpServer"
)]
pub trait RdpServer {
    /// Get the current server status (see [`ServerStatus`] repr).
    fn get_status(&self) -> zbus::Result<u8>;

    /// Tell the daemon to re-read its configuration file.
    fn reload(&self) -> zbus::Result<bool>;

    /// Tell the daemon to shut down gracefully.
    fn stop(&self) -> zbus::Result<bool>;

    /// Whether the server is currently running.
    #[zbus(property)]
    fn running(&self) -> zbus::Result<bool>;

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
