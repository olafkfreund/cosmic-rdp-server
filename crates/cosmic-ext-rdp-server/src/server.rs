use std::num::{NonZeroU16, NonZeroUsize};
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use ironrdp_displaycontrol::pdu::DisplayControlMonitorLayout;
use ironrdp_pdu::input::fast_path::SynchronizeFlags;
use ironrdp_pdu::pointer::PointerPositionAttribute;
use ironrdp_server::{
    BitmapUpdate, CliprdrServerFactory, DesktopSize, DisplayUpdate, KeyboardEvent, MouseEvent,
    PixelFormat, RGBAPointer, RdpServer, RdpServerDisplay, RdpServerDisplayUpdates,
    RdpServerInputHandler, SoundServerFactory,
};
use rdp_capture::{CaptureEvent, CapturedFrame, CursorInfo, DesktopInfo};
use rdp_encode::{EncoderConfig, GstEncoder};
use rdp_input::{EiInput, MouseButton};
use tokio::sync::mpsc;

use crate::egfx::EgfxController;
use crate::tls::TlsContext;

const DEFAULT_WIDTH: u16 = 1920;
const DEFAULT_HEIGHT: u16 = 1080;

// Blue screen color in BGRA format (B=0xCC, G=0x44, R=0x11, A=0xFF)
const BLUE_BGRA: [u8; 4] = [0xCC, 0x44, 0x11, 0xFF];

/// Input handler that logs events but takes no action.
pub struct StaticInputHandler;

impl RdpServerInputHandler for StaticInputHandler {
    fn keyboard(&mut self, event: KeyboardEvent) {
        tracing::debug!(?event, "Keyboard event received");
    }

    fn mouse(&mut self, event: MouseEvent) {
        tracing::trace!(?event, "Mouse event received");
    }
}

// --------------- Live Input (Phase 3 input injection) ---------------

/// Input handler that injects keyboard and mouse events into the compositor.
///
/// Wraps an [`EiInput`] backend and maps all RDP events to
/// the appropriate reis/libei calls.
pub struct LiveInputHandler {
    input: EiInput,
}

impl LiveInputHandler {
    /// Create a new live input handler.
    pub fn new(input: EiInput) -> Self {
        Self { input }
    }
}

impl RdpServerInputHandler for LiveInputHandler {
    fn keyboard(&mut self, event: KeyboardEvent) {
        match event {
            KeyboardEvent::Pressed { code, extended } => {
                self.input.key_press(code, extended);
            }
            KeyboardEvent::Released { code, extended } => {
                self.input.key_release(code, extended);
            }
            // Unicode key events: handle common control characters by mapping
            // them to their corresponding scancode equivalents. Some RDP clients
            // send keys like Backspace, Tab, Enter, and Escape as Unicode
            // character events (U+0008, U+0009, U+000D, U+001B) instead of
            // scancodes, depending on the keyboard input mode.
            //
            // Arbitrary Unicode character injection (e.g. from IME composition)
            // requires zwp_text_input_v3 or a custom keymap, which is deferred
            // until compositor-side support matures (see issue #23).
            KeyboardEvent::UnicodePressed(codepoint) => {
                if let Some((code, extended)) = unicode_to_scancode(codepoint) {
                    self.input.key_press(code, extended);
                } else {
                    tracing::debug!(
                        codepoint,
                        char = %char::from_u32(u32::from(codepoint)).unwrap_or('\u{FFFD}'),
                        "Unicode key press ignored (not yet supported)"
                    );
                }
            }
            KeyboardEvent::UnicodeReleased(codepoint) => {
                if let Some((code, extended)) = unicode_to_scancode(codepoint) {
                    self.input.key_release(code, extended);
                } else {
                    tracing::trace!(
                        codepoint,
                        "Unicode key release ignored (not yet supported)"
                    );
                }
            }
            KeyboardEvent::Synchronize(flags) => {
                let caps = flags.contains(SynchronizeFlags::CAPS_LOCK);
                let num = flags.contains(SynchronizeFlags::NUM_LOCK);
                let scroll = flags.contains(SynchronizeFlags::SCROLL_LOCK);
                self.input.synchronize_locks(caps, num, scroll);
            }
        }
    }

    fn mouse(&mut self, event: MouseEvent) {
        match event {
            MouseEvent::Move { x, y } => {
                self.input.mouse_move(x, y);
            }
            MouseEvent::RelMove { x, y } => {
                self.input.mouse_rel_move(x, y);
            }
            MouseEvent::LeftPressed => {
                self.input.mouse_button(MouseButton::Left, true);
            }
            MouseEvent::LeftReleased => {
                self.input.mouse_button(MouseButton::Left, false);
            }
            MouseEvent::RightPressed => {
                self.input.mouse_button(MouseButton::Right, true);
            }
            MouseEvent::RightReleased => {
                self.input.mouse_button(MouseButton::Right, false);
            }
            MouseEvent::MiddlePressed => {
                self.input.mouse_button(MouseButton::Middle, true);
            }
            MouseEvent::MiddleReleased => {
                self.input.mouse_button(MouseButton::Middle, false);
            }
            MouseEvent::Button4Pressed => {
                self.input.mouse_button(MouseButton::Back, true);
            }
            MouseEvent::Button4Released => {
                self.input.mouse_button(MouseButton::Back, false);
            }
            MouseEvent::Button5Pressed => {
                self.input.mouse_button(MouseButton::Forward, true);
            }
            MouseEvent::Button5Released => {
                self.input.mouse_button(MouseButton::Forward, false);
            }
            MouseEvent::VerticalScroll { value } => {
                self.input.scroll_vertical(i32::from(value));
            }
            MouseEvent::Scroll { x, y } => {
                self.input.scroll(x, y);
            }
        }
    }
}

/// Map a Unicode codepoint to its equivalent RDP XT scancode.
///
/// Some RDP clients send control keys as Unicode character events instead
/// of scancodes. This maps the common control characters back to their
/// XT scancode + extended flag so they can be injected through the normal
/// keycode path.
///
/// Returns `(code, extended)` or `None` if the codepoint has no scancode
/// equivalent.
const fn unicode_to_scancode(codepoint: u16) -> Option<(u8, bool)> {
    match codepoint {
        0x0008 => Some((0x0E, false)), // Backspace
        0x0009 => Some((0x0F, false)), // Tab
        0x000D => Some((0x1C, false)), // Enter / Carriage Return
        0x001B => Some((0x01, false)), // Escape
        0x007F => Some((0x53, true)),  // Delete (extended)
        _ => None,
    }
}

// --------------- Static Display (Phase 1 blue screen) ---------------

/// Display updates that send a single blue bitmap then wait forever.
struct StaticDisplayUpdates {
    receiver: mpsc::Receiver<DisplayUpdate>,
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for StaticDisplayUpdates {
    async fn next_update(&mut self) -> Result<Option<DisplayUpdate>> {
        // Cancellation-safe: mpsc::Receiver::recv is cancel-safe
        Ok(self.receiver.recv().await)
    }
}

/// Static display that returns a fixed resolution and sends a blue bitmap.
pub struct StaticDisplay {
    width: u16,
    height: u16,
}

impl StaticDisplay {
    pub fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

impl Default for StaticDisplay {
    fn default() -> Self {
        Self::new(DEFAULT_WIDTH, DEFAULT_HEIGHT)
    }
}

#[async_trait::async_trait]
impl RdpServerDisplay for StaticDisplay {
    async fn size(&mut self) -> DesktopSize {
        DesktopSize {
            width: self.width,
            height: self.height,
        }
    }

    async fn updates(&mut self) -> Result<Box<dyn RdpServerDisplayUpdates>> {
        let (tx, rx) = mpsc::channel(16);

        let width = self.width;
        let height = self.height;

        // Send the initial blue bitmap on a background task
        tokio::spawn(async move {
            let bitmap = create_blue_bitmap(width, height);
            let update = DisplayUpdate::Bitmap(bitmap);
            if let Err(e) = tx.send(update).await {
                tracing::warn!("Failed to send initial bitmap: {}", e);
            }
            // Keep the sender alive so the channel stays open.
            // The server will keep calling next_update() which will
            // await on recv() indefinitely (no disconnect).
            let () = std::future::pending().await;
        });

        Ok(Box::new(StaticDisplayUpdates { receiver: rx }))
    }
}

fn create_blue_bitmap(width: u16, height: u16) -> BitmapUpdate {
    let w = usize::from(width);
    let h = usize::from(height);
    let bpp = 4; // BGRA = 4 bytes per pixel
    let stride = w * bpp;

    let mut data = vec![0u8; stride * h];
    for pixel in data.chunks_exact_mut(bpp) {
        pixel.copy_from_slice(&BLUE_BGRA);
    }

    BitmapUpdate {
        x: 0,
        y: 0,
        width: NonZeroU16::new(width).expect("width must be non-zero"),
        height: NonZeroU16::new(height).expect("height must be non-zero"),
        format: PixelFormat::BgrA32,
        data: Bytes::from(data),
        stride: NonZeroUsize::new(stride).expect("stride must be non-zero"),
    }
}

// --------------- Live Display (Phase 2 screen capture + Phase 6 resize) -----

/// Shared channel state between [`LiveDisplay`] and [`LiveDisplayUpdates`].
///
/// When a client connects, the receivers are taken from here. When the client
/// disconnects, [`LiveDisplayUpdates::drop`] puts them back so the next
/// connection can reuse them without restarting capture.
struct DisplayChannels {
    event_rx: Option<mpsc::Receiver<CaptureEvent>>,
}

/// Display that streams live screen capture frames via `PipeWire` and
/// supports dynamic resize requests from the RDP client.
///
/// Supports sequential connections: when a client disconnects, the capture
/// channels are returned to shared state so the next client can reuse them.
pub struct LiveDisplay {
    width: u16,
    height: u16,
    channels: Arc<std::sync::Mutex<DisplayChannels>>,
    /// EGFX controller for H.264 delivery and resize (optional).
    /// Retained across connections (cloned into `LiveDisplayUpdates`).
    egfx: Option<EgfxController>,
}

impl LiveDisplay {
    /// Create a live display from a capture event receiver and desktop info.
    ///
    /// The caller must keep the [`rdp_capture::CaptureHandle`] alive for the
    /// duration of the display, otherwise frames will stop arriving.
    pub fn new(event_rx: mpsc::Receiver<CaptureEvent>, info: &DesktopInfo) -> Self {
        Self {
            width: info.width,
            height: info.height,
            channels: Arc::new(std::sync::Mutex::new(DisplayChannels {
                event_rx: Some(event_rx),
            })),
            egfx: None,
        }
    }

    /// Attach an EGFX controller for H.264 frame delivery.
    pub fn set_egfx(&mut self, controller: EgfxController) {
        self.egfx = Some(controller);
    }
}

#[async_trait::async_trait]
impl RdpServerDisplay for LiveDisplay {
    async fn size(&mut self) -> DesktopSize {
        DesktopSize {
            width: self.width,
            height: self.height,
        }
    }

    async fn updates(&mut self) -> Result<Box<dyn RdpServerDisplayUpdates>> {
        let mut channels = self.channels.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        let event_rx = channels
            .event_rx
            .take()
            .ok_or_else(|| anyhow::anyhow!("capture already in use (only one connection at a time)"))?;

        // Clone EGFX controller so LiveDisplay retains access for
        // request_layout() while LiveDisplayUpdates gets its own handle.
        let egfx = self.egfx.clone();

        // Reset EGFX state so the new connection starts with a fresh
        // capability handshake. Without this, stale `ready` / `supports_avc420`
        // flags from a previous connection cause H.264 to be sent without a
        // valid DVC channel.
        if let Some(ref egfx) = egfx {
            egfx.reset();
        }

        tracing::info!("Display channels acquired for new connection");

        Ok(Box::new(LiveDisplayUpdates {
            event_rx: Some(event_rx),
            channels: Arc::clone(&self.channels),
            pending_cursor: None,
            egfx,
            encoder: None,
            encoder_width: 0,
            encoder_height: 0,
            frame_timestamp_ms: 0,
            egfx_wait_frames: 0,
        }))
    }

    fn request_layout(&mut self, layout: DisplayControlMonitorLayout) {
        // Extract the primary monitor dimensions from the layout request.
        let Some(primary) = layout.monitors().iter().find(|m| m.is_primary()) else {
            tracing::debug!("No primary monitor in layout request, ignoring");
            return;
        };

        let (width, height) = primary.dimensions();

        let Ok(width) = u16::try_from(width) else {
            tracing::warn!(width, "Requested width exceeds u16, ignoring resize");
            return;
        };
        let Ok(height) = u16::try_from(height) else {
            tracing::warn!(height, "Requested height exceeds u16, ignoring resize");
            return;
        };

        if width == self.width && height == self.height {
            tracing::debug!(width, height, "Resize requested but dimensions unchanged");
            return;
        }

        // Route resize through EGFX ResetGraphics instead of
        // DisplayUpdate::Resize to avoid ironrdp-server 0.10's broken
        // deactivation-reactivation sequence.
        if let Some(ref egfx) = self.egfx {
            if egfx.is_ready() {
                tracing::info!(
                    width, height,
                    old_width = self.width, old_height = self.height,
                    "Resizing display via EGFX ResetGraphics"
                );
                egfx.resize(width, height);
                self.width = width;
                self.height = height;
                return;
            }
        }

        // EGFX not available or not ready — cannot resize safely.
        tracing::info!(
            width, height,
            "Client requested resize but EGFX not ready, ignoring"
        );
    }
}

/// Display updates that receive live frames from the `PipeWire` capture
/// and handle dynamic resize events from the RDP client.
///
/// When an [`EgfxController`] is present and ready, captured frames are
/// encoded to H.264 via [`GstEncoder`] and delivered through the EGFX
/// DVC channel instead of as raw bitmaps. Falls back to bitmaps when
/// EGFX is not negotiated.
struct LiveDisplayUpdates {
    event_rx: Option<mpsc::Receiver<CaptureEvent>>,
    /// Shared state to return channels to on disconnect.
    channels: Arc<std::sync::Mutex<DisplayChannels>>,
    /// When a `FrameAndCursor` event arrives, we return the frame first
    /// and buffer the cursor update for the next call.
    pending_cursor: Option<CursorInfo>,
    /// EGFX controller for H.264 frame delivery (if available).
    egfx: Option<EgfxController>,
    /// H.264 encoder, lazily initialized on first EGFX frame.
    encoder: Option<GstEncoder>,
    /// Dimensions of the current encoder (0 = not yet initialized).
    encoder_width: u32,
    encoder_height: u32,
    /// Frame timestamp counter (milliseconds), monotonically increasing.
    frame_timestamp_ms: u32,
    /// Frames skipped while waiting for EGFX to become ready.
    /// After a timeout, fall back to bitmap delivery even if EGFX never
    /// negotiates (e.g. client connected with /gfx:off).
    egfx_wait_frames: u32,
}

impl Drop for LiveDisplayUpdates {
    fn drop(&mut self) {
        let mut channels = self.channels.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        channels.event_rx = self.event_rx.take();
        // EGFX controller is not returned — LiveDisplay retains its own clone.
        // Drop the encoder to release GStreamer resources.
        self.encoder = None;
        tracing::info!("Client disconnected, display channels released for next connection");
    }
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for LiveDisplayUpdates {
    async fn next_update(&mut self) -> Result<Option<DisplayUpdate>> {
        // If we have a buffered cursor update from a previous FrameAndCursor,
        // return it immediately before reading more events.
        if let Some(cursor) = self.pending_cursor.take() {
            return Ok(Some(cursor_to_display_update(&cursor)));
        }

        let event_rx = self.event_rx.as_mut().expect("event_rx missing during active connection");

        loop {
            let Some(event) = event_rx.recv().await else {
                return Ok(None);
            };

            match event {
                CaptureEvent::Frame(mut frame) => {
                    frame.ensure_alpha_opaque();
                    if try_send_egfx_frame(
                        self.egfx.as_ref(),
                        &mut self.encoder,
                        &mut self.encoder_width,
                        &mut self.encoder_height,
                        &mut self.frame_timestamp_ms,
                        &frame,
                    ) {
                        continue;
                    }
                    // When EGFX is configured, skip bitmap fallback while the
                    // DVC channel is still negotiating. Sending bitmaps at the
                    // capture resolution (e.g. 1920x1080) crashes FreeRDP if
                    // the client's desktop is smaller (e.g. 1662x860):
                    //   "Invalid surface bits command rectangle does not fit"
                    // After ~300 frames (~10s at 30fps) fall back to bitmap
                    // for clients that don't support EGFX.
                    if self.egfx.is_some() && self.egfx_wait_frames < 300 {
                        self.egfx_wait_frames += 1;
                        if self.egfx_wait_frames == 1 {
                            tracing::info!(
                                "EGFX not yet ready, suppressing bitmap fallback"
                            );
                        }
                        continue;
                    }
                    let bitmap = frame_to_bitmap(frame)?;
                    return Ok(Some(DisplayUpdate::Bitmap(bitmap)));
                }
                CaptureEvent::Cursor(cursor) => {
                    return Ok(Some(cursor_to_display_update(&cursor)));
                }
                CaptureEvent::FrameAndCursor(mut frame, cursor) => {
                    self.pending_cursor = Some(cursor);
                    frame.ensure_alpha_opaque();
                    if try_send_egfx_frame(
                        self.egfx.as_ref(),
                        &mut self.encoder,
                        &mut self.encoder_width,
                        &mut self.encoder_height,
                        &mut self.frame_timestamp_ms,
                        &frame,
                    ) {
                        continue;
                    }
                    if self.egfx.is_some() && self.egfx_wait_frames < 300 {
                        self.egfx_wait_frames += 1;
                        continue;
                    }
                    let bitmap = frame_to_bitmap(frame)?;
                    return Ok(Some(DisplayUpdate::Bitmap(bitmap)));
                }
            }
        }
    }
}

/// Try to encode a frame as H.264 and send it via EGFX.
///
/// Returns `true` if the frame was sent via EGFX (caller should skip
/// bitmap delivery), `false` if EGFX is not ready and bitmap fallback
/// should be used.
///
/// Detects frame dimension changes (from `PipeWire` resolution changes or
/// EGFX resize) and recreates the encoder to match.
#[allow(clippy::cast_possible_truncation)]
fn try_send_egfx_frame(
    egfx: Option<&EgfxController>,
    h264_encoder: &mut Option<GstEncoder>,
    encoder_width: &mut u32,
    encoder_height: &mut u32,
    timestamp_ms: &mut u32,
    frame: &CapturedFrame,
) -> bool {
    let Some(egfx) = egfx else {
        return false;
    };

    if !egfx.is_ready() || !egfx.supports_avc420() {
        return false;
    }

    // Detect frame dimension change: drop encoder so it gets recreated
    // at the new size. This handles both client-initiated resize (via
    // EGFX ResetGraphics in request_layout) and PipeWire resolution changes.
    if h264_encoder.is_some() && (frame.width != *encoder_width || frame.height != *encoder_height) {
        tracing::info!(
            old_width = *encoder_width, old_height = *encoder_height,
            new_width = frame.width, new_height = frame.height,
            "EGFX: frame dimensions changed, recreating encoder"
        );
        *h264_encoder = None;

        // Ensure the EGFX surface matches the new frame dimensions.
        egfx.resize(frame.width as u16, frame.height as u16);
    }

    // Lazily initialize the H.264 encoder on the first EGFX frame or
    // after a dimension change.
    if h264_encoder.is_none() {
        let config = EncoderConfig {
            width: frame.width,
            height: frame.height,
            ..EncoderConfig::default()
        };
        match GstEncoder::new(&config) {
            Ok(enc) => {
                tracing::info!(
                    width = frame.width,
                    height = frame.height,
                    encoder_type = %enc.encoder_type(),
                    "EGFX: H.264 encoder initialized"
                );
                // Force a keyframe if EGFX was resized, ensuring the
                // client can decode immediately after surface recreation.
                if egfx.take_needs_keyframe() {
                    enc.force_keyframe();
                }
                *encoder_width = frame.width;
                *encoder_height = frame.height;
                *h264_encoder = Some(enc);
            }
            Err(e) => {
                tracing::warn!("EGFX: failed to initialize H.264 encoder: {e}, falling back to bitmap");
                return false;
            }
        }
    }

    let enc = h264_encoder.as_mut().expect("encoder just initialized");

    match enc.encode_frame(&frame.data) {
        Ok(Some(h264_frame)) => {
            let width = frame.width as u16;
            let height = frame.height as u16;
            let ts = *timestamp_ms;
            *timestamp_ms = timestamp_ms.wrapping_add(33); // ~30 fps

            egfx.send_frame(&h264_frame.data, width, height, ts)
        }
        Ok(None) => {
            // Encoder is buffering, no output yet — fall back to bitmap
            // for this frame so the client isn't starved.
            false
        }
        Err(e) => {
            tracing::warn!("EGFX: H.264 encoding failed: {e}, falling back to bitmap");
            false
        }
    }
}

/// Convert a [`CursorInfo`] to the appropriate [`DisplayUpdate`] variant.
fn cursor_to_display_update(cursor: &CursorInfo) -> DisplayUpdate {
    if !cursor.visible {
        return DisplayUpdate::HidePointer;
    }

    if let Some(ref bitmap) = cursor.bitmap {
        #[allow(clippy::cast_possible_truncation)]
        DisplayUpdate::RGBAPointer(RGBAPointer {
            width: bitmap.width as u16,
            height: bitmap.height as u16,
            hot_x: bitmap.hot_x as u16,
            hot_y: bitmap.hot_y as u16,
            data: bitmap.data.clone(),
        })
    } else {
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        DisplayUpdate::PointerPosition(PointerPositionAttribute {
            x: cursor.x.max(0) as u16,
            y: cursor.y.max(0) as u16,
        })
    }
}

/// Convert a captured frame to an ironrdp `BitmapUpdate`.
fn frame_to_bitmap(frame: CapturedFrame) -> Result<BitmapUpdate> {
    let width = u16::try_from(frame.width)
        .map_err(|_| anyhow::anyhow!("frame width {} exceeds u16", frame.width))?;
    let height = u16::try_from(frame.height)
        .map_err(|_| anyhow::anyhow!("frame height {} exceeds u16", frame.height))?;

    let width =
        NonZeroU16::new(width).ok_or_else(|| anyhow::anyhow!("frame width is zero"))?;
    let height =
        NonZeroU16::new(height).ok_or_else(|| anyhow::anyhow!("frame height is zero"))?;
    let stride = NonZeroUsize::new(frame.stride as usize)
        .ok_or_else(|| anyhow::anyhow!("frame stride is zero"))?;

    Ok(BitmapUpdate {
        x: 0,
        y: 0,
        width,
        height,
        format: PixelFormat::BgrA32,
        data: Bytes::from(frame.data),
        stride,
    })
}

// --------------- Authentication ---------------

/// NLA authentication credentials.
pub struct AuthCredentials {
    /// Username.
    pub username: String,
    /// Password.
    pub password: String,
    /// Windows domain (optional).
    pub domain: Option<String>,
}

// --------------- Server Builders ---------------

/// Macro to apply TLS or Hybrid security and return the builder at the
/// `WantsHandler` stage. We use a macro because the intermediate builder
/// types (`WantsSecurity`, `WantsHandler`) are not re-exported by
/// `ironrdp-server`.
macro_rules! with_security {
    ($builder:expr, $tls:expr, $auth:expr) => {
        if $auth.is_some() {
            $builder.with_hybrid($tls.acceptor.clone(), $tls.public_key.clone())
        } else {
            $builder.with_tls($tls.acceptor.clone())
        }
    };
}

/// Build an RDP server with the static blue screen display (fallback).
pub fn build_server(
    bind_addr: std::net::SocketAddr,
    tls: &TlsContext,
    auth: Option<&AuthCredentials>,
    cliprdr: Option<Box<dyn CliprdrServerFactory>>,
    sound: Option<Box<dyn SoundServerFactory>>,
    egfx_factory: Option<Box<dyn ironrdp_dvc::DvcProcessorFactory>>,
) -> RdpServer {
    let builder = RdpServer::builder().with_addr(bind_addr);
    let builder = with_security!(builder, tls, auth);
    let mut server = builder
        .with_input_handler(StaticInputHandler)
        .with_display_handler(StaticDisplay::default())
        .with_cliprdr_factory(cliprdr)
        .with_sound_factory(sound)
        .build();
    apply_credentials(&mut server, auth);
    if let Some(factory) = egfx_factory {
        server.add_dvc_factory(factory);
    }
    server
}

/// Build an RDP server with live screen capture and input injection.
///
/// If `egfx_factory` is provided, it is registered as a DVC processor factory
/// for EGFX/H.264 frame delivery through the DRDYNVC channel. A fresh
/// processor is created for each RDP connection.
#[allow(clippy::too_many_arguments)]
pub fn build_live_server(
    bind_addr: std::net::SocketAddr,
    tls: &TlsContext,
    auth: Option<&AuthCredentials>,
    display: LiveDisplay,
    input_handler: LiveInputHandler,
    cliprdr: Option<Box<dyn CliprdrServerFactory>>,
    sound: Option<Box<dyn SoundServerFactory>>,
    egfx_factory: Option<Box<dyn ironrdp_dvc::DvcProcessorFactory>>,
) -> RdpServer {
    let builder = RdpServer::builder().with_addr(bind_addr);
    let builder = with_security!(builder, tls, auth);
    let mut server = builder
        .with_input_handler(input_handler)
        .with_display_handler(display)
        .with_cliprdr_factory(cliprdr)
        .with_sound_factory(sound)
        .build();
    apply_credentials(&mut server, auth);
    if let Some(factory) = egfx_factory {
        server.add_dvc_factory(factory);
    }
    server
}

/// Build an RDP server with live capture but no input injection (view-only).
pub fn build_view_only_server(
    bind_addr: std::net::SocketAddr,
    tls: &TlsContext,
    auth: Option<&AuthCredentials>,
    display: LiveDisplay,
    cliprdr: Option<Box<dyn CliprdrServerFactory>>,
    sound: Option<Box<dyn SoundServerFactory>>,
) -> RdpServer {
    let builder = RdpServer::builder().with_addr(bind_addr);
    let builder = with_security!(builder, tls, auth);
    let mut server = builder
        .with_input_handler(StaticInputHandler)
        .with_display_handler(display)
        .with_cliprdr_factory(cliprdr)
        .with_sound_factory(sound)
        .build();
    apply_credentials(&mut server, auth);
    server
}

/// Set credentials on the server.
///
/// ironrdp-acceptor always validates `ClientInfoPdu` credentials, even in
/// TLS-only mode.  When NLA auth is disabled we set empty credentials so
/// that clients connecting with an empty username/password are accepted.
fn apply_credentials(server: &mut RdpServer, auth: Option<&AuthCredentials>) {
    if let Some(auth) = auth {
        let creds = ironrdp_server::Credentials {
            username: auth.username.clone(),
            password: auth.password.clone(),
            domain: auth.domain.clone(),
        };
        server.set_credentials(Some(creds));
        tracing::info!(username = %auth.username, "NLA credentials configured");
    } else {
        // ironrdp-acceptor rejects connections when server credentials are
        // None because `None != Some(client_creds)`.  Set empty credentials
        // so unauthenticated clients can connect with empty user/password.
        let creds = ironrdp_server::Credentials {
            username: String::new(),
            password: String::new(),
            domain: None,
        };
        server.set_credentials(Some(creds));
        tracing::info!("No auth configured; accepting empty credentials");
    }
}
