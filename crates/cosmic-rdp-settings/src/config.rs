use std::path::PathBuf;

use anyhow::{Context, Result};
use rdp_dbus::config::ServerConfig;

/// Default config directory under `$XDG_CONFIG_HOME`.
const CONFIG_DIR: &str = "cosmic-rdp-server";
/// Default config file name.
const CONFIG_FILE: &str = "config.toml";

/// Resolve the default config file path.
///
/// Returns `$XDG_CONFIG_HOME/cosmic-rdp-server/config.toml` or
/// `~/.config/cosmic-rdp-server/config.toml`.
#[must_use]
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join(CONFIG_DIR)
        .join(CONFIG_FILE)
}

/// Load the server configuration from the default TOML location.
///
/// Returns the default configuration if the file does not exist.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load() -> Result<ServerConfig> {
    let path = config_path();

    if !path.exists() {
        return Ok(ServerConfig::default());
    }

    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config: {}", path.display()))?;

    let config: ServerConfig = toml::from_str(&contents)
        .with_context(|| format!("failed to parse config: {}", path.display()))?;

    Ok(config)
}

/// Save the server configuration to the default TOML location.
///
/// Creates the parent directory if it does not exist.
///
/// # Errors
///
/// Returns an error if the file cannot be written.
pub fn save(config: &ServerConfig) -> Result<()> {
    let path = config_path();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config dir: {}", parent.display()))?;
    }

    let contents = toml::to_string_pretty(config).context("failed to serialize config")?;

    std::fs::write(&path, contents)
        .with_context(|| format!("failed to write config: {}", path.display()))?;

    tracing::info!(?path, "Configuration saved");
    Ok(())
}
