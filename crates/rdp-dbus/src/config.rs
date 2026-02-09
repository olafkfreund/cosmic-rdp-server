use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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

/// Load the server configuration from a TOML file.
///
/// If `path` is `None`, reads from the default location.
/// Returns the default configuration if the file does not exist.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load(path: Option<&Path>) -> Result<ServerConfig> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => config_path(),
    };

    if !path.exists() {
        tracing::debug!(?path, "Config file not found, using defaults");
        return Ok(ServerConfig::default());
    }

    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config: {}", path.display()))?;

    let config: ServerConfig = toml::from_str(&contents)
        .with_context(|| format!("failed to parse config: {}", path.display()))?;

    tracing::info!(?path, "Configuration loaded");
    Ok(config)
}

/// Save the server configuration to the default TOML location.
///
/// Uses atomic write (write to temp file, then rename) to prevent
/// partial writes. Creates the parent directory if it does not exist.
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

    // Atomic write: write to temp file, then rename.
    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, contents)
        .with_context(|| format!("failed to write temp config: {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &path)
        .with_context(|| format!("failed to rename config: {}", path.display()))?;

    tracing::info!(?path, "Configuration saved");
    Ok(())
}

/// Server configuration loaded from TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Audio forwarding settings.
    pub audio: AudioConfig,
}

/// NLA authentication configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    /// Target frames per second.
    pub fps: u32,

    /// `PipeWire` channel capacity (number of buffered frames).
    pub channel_capacity: usize,

    /// Enable multi-monitor capture (merges all selected monitors into
    /// a single virtual desktop).
    pub multi_monitor: bool,
}

/// Audio forwarding settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Enable RDPSND audio forwarding.
    pub enable: bool,

    /// Audio sample rate in Hz.
    pub sample_rate: u32,

    /// Number of audio channels.
    pub channels: u16,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enable: true,
            sample_rate: 44100,
            channels: 2,
        }
    }
}

/// Clipboard sharing settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
            audio: AudioConfig::default(),
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            channel_capacity: 4,
            multi_monitor: false,
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
