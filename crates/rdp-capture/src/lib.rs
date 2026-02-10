//! Screen capture abstraction for cosmic-rdp-server.
//!
//! Provides screen capture via the XDG `ScreenCast` portal and `PipeWire`.
//!
//! Use [`start_capture`] for a high-level API that handles portal negotiation
//! and `PipeWire` stream setup.

pub mod audio_stream;
pub mod compositor;
pub mod frame;
pub mod pipewire_stream;
pub mod portal;
pub mod spa_meta;

pub use audio_stream::{AudioCaptureError, PwAudioStream};
pub use compositor::{bounding_box, FrameCompositor, MonitorInfo};
pub use frame::{
    AudioChunk, CaptureEvent, CapturedFrame, CursorBitmap, CursorInfo, DamageRect, PixelFormat,
};
pub use pipewire_stream::{PwError, PwStream};
pub use portal::{start_screencast, PortalError, PortalSession, PortalStream};

use ashpd::desktop::screencast::Screencast;
use tokio::sync::mpsc;

/// Information about the captured desktop.
#[derive(Debug, Clone)]
pub struct DesktopInfo {
    /// Desktop width in pixels.
    pub width: u16,
    /// Desktop height in pixels.
    pub height: u16,
    /// `PipeWire` node ID.
    pub node_id: u32,
    /// Restore token for reconnecting to the same session.
    pub restore_token: Option<String>,
}

/// Handle that keeps the capture session alive.
///
/// Dropping this stops the `PipeWire` stream and releases the portal session.
/// Must be kept alive for the duration of the capture.
pub struct CaptureHandle {
    _session: ashpd::desktop::Session<'static, Screencast<'static>>,
    _proxy: Screencast<'static>,
    _pw_stream: PwStream,
}

/// Start a screen capture session: portal negotiation + `PipeWire` stream.
///
/// Shows the system permission dialog if no valid `restore_token` is provided.
/// Returns a handle (must be kept alive), a receiver for captured frames,
/// and information about the captured desktop.
///
/// # Errors
///
/// Returns `CaptureError` if the portal session or `PipeWire` stream fails.
pub async fn start_capture(
    restore_token: Option<&str>,
    channel_capacity: usize,
) -> Result<(CaptureHandle, mpsc::Receiver<CaptureEvent>, DesktopInfo), CaptureError> {
    let portal_session = start_screencast(restore_token, true, false)
        .await
        .map_err(CaptureError::Portal)?;

    let stream = &portal_session.streams[0];
    let info = DesktopInfo {
        width: stream
            .width
            .and_then(|w| u16::try_from(w).ok())
            .unwrap_or(1920),
        height: stream
            .height
            .and_then(|h| u16::try_from(h).ok())
            .unwrap_or(1080),
        node_id: stream.node_id,
        restore_token: portal_session.restore_token.clone(),
    };

    let PortalSession {
        session,
        proxy,
        streams: _,
        restore_token: _,
        pipewire_fd,
    } = portal_session;

    let (pw_stream, frame_rx) = PwStream::start(pipewire_fd, info.node_id, channel_capacity)
        .map_err(CaptureError::PipeWire)?;

    let handle = CaptureHandle {
        _session: session,
        _proxy: proxy,
        _pw_stream: pw_stream,
    };

    tracing::info!(
        width = info.width,
        height = info.height,
        node_id = info.node_id,
        "Screen capture session started"
    );

    Ok((handle, frame_rx, info))
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("ScreenCast portal session failed")]
    Portal(#[source] PortalError),

    #[error("PipeWire stream failed")]
    PipeWire(#[source] PwError),
}
