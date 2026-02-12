use anyhow::{Context, Result};
use zbus::interface;

use crate::session::SessionRegistry;

/// D-Bus interface for the COSMIC RDP Broker on the system bus.
pub struct BrokerInterface {
    registry: SessionRegistry,
}

impl BrokerInterface {
    #[must_use]
    pub fn new(registry: SessionRegistry) -> Self {
        Self { registry }
    }
}

#[interface(name = "io.github.olafkfreund.CosmicExtRdpBroker")]
impl BrokerInterface {
    /// List all active sessions as JSON.
    async fn list_sessions(&self) -> String {
        let sessions = self.registry.list().await;
        serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_string())
    }

    /// Get the number of active sessions.
    #[allow(clippy::cast_possible_truncation)]
    async fn active_session_count(&self) -> u32 {
        self.registry.count().await as u32
    }

    /// Terminate a specific user's session.
    async fn terminate_session(&self, username: &str) -> bool {
        if let Some(entry) = self.registry.remove(username).await {
            if let Err(e) = crate::spawner::stop_user_server(&entry.unit_name).await {
                tracing::warn!(username, "Failed to stop session: {e}");
            }
            if let Err(e) = self.registry.save_state().await {
                tracing::warn!("Failed to save state after termination: {e}");
            }
            true
        } else {
            false
        }
    }
}

/// Start the D-Bus server on the system bus.
///
/// Registers the broker interface at the well-known name and object path.
pub async fn start_broker_dbus(registry: SessionRegistry) -> Result<zbus::Connection> {
    let iface = BrokerInterface::new(registry);

    let connection = zbus::connection::Builder::system()
        .context("failed to connect to system D-Bus")?
        .name(rdp_dbus::constants::BROKER_SERVICE_NAME)
        .context("failed to request broker D-Bus name")?
        .serve_at(rdp_dbus::constants::BROKER_OBJECT_PATH, iface)
        .context("failed to serve broker D-Bus interface")?
        .build()
        .await
        .context("failed to build broker D-Bus connection")?;

    tracing::info!(
        service = rdp_dbus::constants::BROKER_SERVICE_NAME,
        path = rdp_dbus::constants::BROKER_OBJECT_PATH,
        "Broker D-Bus interface registered on system bus"
    );

    Ok(connection)
}
