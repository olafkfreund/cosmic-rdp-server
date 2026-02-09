// Screen capture abstraction for cosmic-rdp-server.
//
// Provides the CaptureSource trait and implementations:
// - portal.rs: ScreenCast portal via ashpd
// - pipewire.rs: PipeWire stream handler
// - frame.rs: VideoFrame type and pixel formats

pub mod frame;
pub mod pipewire_stream;
pub mod portal;
