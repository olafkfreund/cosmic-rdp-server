use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Server configuration loaded from TOML file.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Network bind address and port.
    pub bind: SocketAddr,

    /// TLS certificate path (PEM). If absent, generate self-signed.
    pub cert_path: Option<PathBuf>,

    /// TLS private key path (PEM). Required if `cert_path` is set.
    pub key_path: Option<PathBuf>,

    /// Use a static blue screen instead of live capture.
    pub static_display: bool,

    /// Authentication settings.
    pub auth: AuthConfig,

    /// Capture settings.
    pub capture: CaptureConfig,

    /// Encoding settings.
    pub encode: EncodeConfig,

    /// Clipboard settings.
    pub clipboard: ClipboardConfig,
}

/// NLA authentication configuration.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Enable NLA (Network Level Authentication) via `CredSSP`.
    /// When enabled, clients must authenticate before seeing the desktop.
    pub enable: bool,

    /// Username for NLA authentication.
    pub username: String,

    /// Password for NLA authentication.
    /// Consider using a secrets file instead of storing in the config.
    pub password: String,

    /// Windows domain (optional).
    pub domain: Option<String>,
}

/// Screen capture settings.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    /// Target frames per second.
    pub fps: u32,

    /// `PipeWire` channel capacity (number of buffered frames).
    pub channel_capacity: usize,
}

/// Clipboard sharing settings.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ClipboardConfig {
    /// Enable clipboard sharing between local and remote sessions.
    pub enable: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self { enable: true }
    }
}

/// Video encoding settings.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct EncodeConfig {
    /// Preferred encoder: "vaapi", "nvenc", "software", or "auto".
    pub encoder: String,

    /// H.264 encoding preset.
    pub preset: String,

    /// Target bitrate in bits per second.
    pub bitrate: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:3389".parse().expect("valid default address"),
            cert_path: None,
            key_path: None,
            static_display: false,
            auth: AuthConfig::default(),
            capture: CaptureConfig::default(),
            encode: EncodeConfig::default(),
            clipboard: ClipboardConfig::default(),
        }
    }
}


impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            channel_capacity: 4,
        }
    }
}

impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            encoder: "auto".to_string(),
            preset: "ultrafast".to_string(),
            bitrate: 10_000_000,
        }
    }
}

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
