use std::path::Path;

use anyhow::{Context, Result};

// Re-export config types from the shared crate.
pub use rdp_dbus::config::ServerConfig;

/// Load configuration from a TOML file.
///
/// Returns the default configuration if the path is `None` or the file
/// does not exist.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_config(path: Option<&Path>) -> Result<ServerConfig> {
    let Some(path) = path else {
        tracing::debug!("No config file specified, using defaults");
        return Ok(ServerConfig::default());
    };

    if !path.exists() {
        tracing::debug!(?path, "Config file not found, using defaults");
        return Ok(ServerConfig::default());
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;

    let config: ServerConfig = toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file: {}", path.display()))?;

    tracing::info!(?path, "Configuration loaded");
    Ok(config)
}
