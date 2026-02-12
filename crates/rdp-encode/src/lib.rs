//! Video encoding abstraction for cosmic-ext-rdp-server.
//!
//! Provides H.264 encoding via `GStreamer` with automatic hardware
//! encoder detection (VAAPI, NVENC, x264 software fallback).
//!
//! The encoder is designed as a standalone component that can be
//! integrated with ironrdp-server's EGFX channel when available,
//! or used for server-side frame processing.
//!
//! - [`gstreamer_enc`]: H.264 encoding via `GStreamer` pipeline
//! - [`bitmap`]: Raw bitmap pass-through (no encoding)

pub mod bitmap;
pub mod gstreamer_enc;

pub use bitmap::BitmapEncoder;
pub use gstreamer_enc::{EncoderType, GstEncoder};

/// Configuration for the video encoder.
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// Video width in pixels.
    pub width: u32,
    /// Video height in pixels.
    pub height: u32,
    /// Target bitrate in bits per second.
    pub bitrate: u32,
    /// Target frame rate.
    pub framerate: u32,
    /// Encoder type to use. `None` means auto-detect best available.
    pub encoder_type: Option<EncoderType>,
    /// Enable low-latency mode (zerolatency tune, ultrafast preset).
    pub low_latency: bool,
    /// Keyframe interval in frames (GOP size).
    pub keyframe_interval: u32,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            bitrate: 10_000_000, // 10 Mbps
            framerate: 30,
            encoder_type: None, // auto-detect
            low_latency: true,
            keyframe_interval: 30,
        }
    }
}

/// An H.264 encoded frame ready for delivery.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// H.264 NAL units in byte-stream format (Annex B).
    pub data: Vec<u8>,
    /// Presentation timestamp in microseconds.
    pub pts: u64,
    /// Frame duration in microseconds.
    pub duration: u64,
    /// Whether this is an IDR keyframe.
    pub is_keyframe: bool,
}

/// Errors from the encoding pipeline.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    /// `GStreamer` initialization failed.
    #[error("GStreamer initialization failed: {0}")]
    GstInit(String),

    /// Failed to create a `GStreamer` element.
    #[error("failed to create GStreamer element '{name}': {reason}")]
    ElementCreate {
        /// Element name.
        name: String,
        /// Reason.
        reason: String,
    },

    /// Failed to link `GStreamer` pipeline elements.
    #[error("failed to link GStreamer pipeline: {0}")]
    PipelineLink(String),

    /// Pipeline state change failed.
    #[error("pipeline state change failed: {0}")]
    StateChange(String),

    /// Failed to push a buffer into the pipeline.
    #[error("failed to push buffer to encoder: {0}")]
    PushBuffer(String),

    /// Failed to map a `GStreamer` buffer.
    #[error("failed to map GStreamer buffer")]
    BufferMap,
}
