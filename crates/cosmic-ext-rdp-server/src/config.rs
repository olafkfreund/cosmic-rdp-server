use std::path::Path;

use anyhow::Result;

// Re-export config types from the shared crate.
pub use rdp_dbus::config::ServerConfig;

/// Load configuration from a TOML file.
///
/// If `path` is `None`, falls back to the default XDG location.
/// Returns the default configuration if the file does not exist.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_config(path: Option<&Path>) -> Result<ServerConfig> {
    rdp_dbus::config::load(path)
}
