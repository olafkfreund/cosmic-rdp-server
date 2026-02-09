use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

/// Current status of the RDP server daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[repr(u8)]
pub enum ServerStatus {
    /// Server is stopped / not running.
    Stopped = 0,
    /// Server is starting up.
    Starting = 1,
    /// Server is running and accepting connections.
    Running = 2,
    /// Server encountered an error.
    Error = 3,
}

impl std::fmt::Display for ServerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "Stopped"),
            Self::Starting => write!(f, "Starting"),
            Self::Running => write!(f, "Running"),
            Self::Error => write!(f, "Error"),
        }
    }
}

/// Information about a connected RDP client.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ClientInfo {
    /// Remote address of the client.
    pub address: String,
    /// Unix timestamp (seconds) when the client connected.
    pub connected_at: i64,
}
