// Video encoding abstraction for cosmic-rdp-server.
//
// Provides the FrameEncoder trait and implementations:
// - gstreamer.rs: H.264 encoding via GStreamer (VAAPI/NVENC/software)
// - bitmap.rs: Raw bitmap fallback for clients without EGFX

pub mod bitmap;
pub mod gstreamer_enc;
