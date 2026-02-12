use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Broker configuration loaded from TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrokerConfig {
    /// Address to bind the broker listener to.
    pub bind: String,

    /// Path to the per-user `cosmic-ext-rdp-server` binary.
    pub server_binary: PathBuf,

    /// Start of the port range allocated to per-user sessions.
    pub port_range_start: u16,

    /// End of the port range (inclusive) for per-user sessions.
    pub port_range_end: u16,

    /// PAM service name used for authentication.
    pub pam_service: String,

    /// Seconds of idle time before a disconnected session is terminated.
    pub idle_timeout_secs: u64,

    /// Maximum number of concurrent sessions.
    pub max_sessions: usize,

    /// Session policy: what to do when a user already has a session.
    pub session_policy: SessionPolicy,

    /// Path to the persisted session state file (JSON).
    pub state_file: PathBuf,

    /// Path to TLS certificate (PEM). If absent, generate self-signed per-user.
    pub cert_path: Option<PathBuf>,

    /// Path to TLS private key (PEM). Required if `cert_path` is set.
    pub key_path: Option<PathBuf>,
}

/// Policy for handling existing sessions when a user reconnects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionPolicy {
    /// Only one session per user. Reconnect to existing session.
    OnePerUser,
    /// Disconnect the existing session and create a new one.
    ReplaceExisting,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:3389".to_string(),
            server_binary: PathBuf::from("/usr/bin/cosmic-ext-rdp-server"),
            port_range_start: 3390,
            port_range_end: 3489,
            pam_service: "cosmic-ext-rdp".to_string(),
            idle_timeout_secs: 3600,
            max_sessions: 100,
            session_policy: SessionPolicy::OnePerUser,
            state_file: PathBuf::from("/var/lib/cosmic-ext-rdp-broker/sessions.json"),
            cert_path: None,
            key_path: None,
        }
    }
}

/// Load the broker configuration from a TOML file.
///
/// Returns the default configuration if the file does not exist.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load(path: Option<&Path>) -> Result<BrokerConfig> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from("/etc/cosmic-ext-rdp-broker/config.toml"),
    };

    if !path.exists() {
        tracing::debug!(?path, "Broker config not found, using defaults");
        return Ok(BrokerConfig::default());
    }

    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read broker config: {}", path.display()))?;

    let config: BrokerConfig = toml::from_str(&contents)
        .with_context(|| format!("failed to parse broker config: {}", path.display()))?;

    tracing::info!(?path, "Broker configuration loaded");
    Ok(config)
}
