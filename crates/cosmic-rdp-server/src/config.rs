use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Server configuration loaded from TOML file.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Network bind address and port
    pub bind: SocketAddr,

    /// TLS certificate path (PEM). If absent, generate self-signed.
    pub cert_path: Option<PathBuf>,

    /// TLS private key path (PEM). Required if `cert_path` is set.
    pub key_path: Option<PathBuf>,

    /// Capture settings
    pub capture: CaptureConfig,

    /// Encoding settings
    pub encode: EncodeConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    /// Target frames per second
    pub fps: u32,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct EncodeConfig {
    /// Preferred encoder: "vaapi", "nvenc", "software", or "auto"
    pub encoder: String,

    /// H.264 encoding preset
    pub preset: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:3389".parse().unwrap(),
            cert_path: None,
            key_path: None,
            capture: CaptureConfig::default(),
            encode: EncodeConfig::default(),
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { fps: 30 }
    }
}

impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            encoder: "auto".to_string(),
            preset: "ultrafast".to_string(),
        }
    }
}
