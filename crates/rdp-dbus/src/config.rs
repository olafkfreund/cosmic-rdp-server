use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
