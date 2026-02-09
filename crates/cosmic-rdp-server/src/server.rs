use std::num::{NonZeroU16, NonZeroUsize};

use anyhow::Result;
use bytes::Bytes;
use ironrdp_displaycontrol::pdu::DisplayControlMonitorLayout;
use ironrdp_server::{
    BitmapUpdate, CliprdrServerFactory, DesktopSize, DisplayUpdate, KeyboardEvent, MouseEvent,
    PixelFormat, RGBAPointer, RdpServer, RdpServerDisplay, RdpServerDisplayUpdates,
    RdpServerInputHandler, SoundServerFactory,
};
use ironrdp_pdu::pointer::PointerPositionAttribute;
use enigo::{Button, Direction};
use rdp_capture::{CaptureEvent, CapturedFrame, CursorInfo, DesktopInfo};
use rdp_input::EnigoInput;
use tokio::sync::mpsc;

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
/// Wraps an [`EnigoInput`] backend and maps all RDP events to
/// the appropriate enigo/libei calls.
pub struct LiveInputHandler {
    input: EnigoInput,
}

impl LiveInputHandler {
    /// Create a new live input handler.
    ///
    /// # Errors
    ///
    /// Returns an error if the input backend cannot connect to the compositor.
    pub fn new(input: EnigoInput) -> Self {
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
            KeyboardEvent::UnicodePressed(_) | KeyboardEvent::UnicodeReleased(_) => {
                // Unicode key events are not yet supported.
                // These require XKB compose / text input protocol integration.
                tracing::debug!(?event, "Unicode key event ignored (not yet supported)");
            }
            KeyboardEvent::Synchronize(_flags) => {
                // Lock key sync (Caps Lock, Num Lock, Scroll Lock).
                // TODO: Read current LED state and toggle to match.
                tracing::debug!(?event, "Key synchronize event ignored (not yet supported)");
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
                self.input.mouse_button(Button::Left, Direction::Press);
            }
            MouseEvent::LeftReleased => {
                self.input.mouse_button(Button::Left, Direction::Release);
            }
            MouseEvent::RightPressed => {
                self.input.mouse_button(Button::Right, Direction::Press);
            }
            MouseEvent::RightReleased => {
                self.input.mouse_button(Button::Right, Direction::Release);
            }
            MouseEvent::MiddlePressed => {
                self.input.mouse_button(Button::Middle, Direction::Press);
            }
            MouseEvent::MiddleReleased => {
                self.input.mouse_button(Button::Middle, Direction::Release);
            }
            MouseEvent::Button4Pressed => {
                self.input.mouse_button(Button::Back, Direction::Press);
            }
            MouseEvent::Button4Released => {
                self.input.mouse_button(Button::Back, Direction::Release);
            }
            MouseEvent::Button5Pressed => {
                self.input.mouse_button(Button::Forward, Direction::Press);
            }
            MouseEvent::Button5Released => {
                self.input.mouse_button(Button::Forward, Direction::Release);
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

/// Display that streams live screen capture frames via `PipeWire` and
/// supports dynamic resize requests from the RDP client.
pub struct LiveDisplay {
    width: u16,
    height: u16,
    event_rx: Option<mpsc::Receiver<CaptureEvent>>,
    /// Sender half of the resize channel. When the RDP client requests a
    /// layout change, we send the new size here; the `LiveDisplayUpdates`
    /// picks it up and emits `DisplayUpdate::Resize`.
    resize_tx: mpsc::Sender<DesktopSize>,
    /// Receiver passed to `LiveDisplayUpdates` on first call to `updates()`.
    resize_rx: Option<mpsc::Receiver<DesktopSize>>,
}

impl LiveDisplay {
    /// Create a live display from a capture event receiver and desktop info.
    ///
    /// The caller must keep the [`rdp_capture::CaptureHandle`] alive for the
    /// duration of the display, otherwise frames will stop arriving.
    pub fn new(event_rx: mpsc::Receiver<CaptureEvent>, info: &DesktopInfo) -> Self {
        let (resize_tx, resize_rx) = mpsc::channel(4);
        Self {
            width: info.width,
            height: info.height,
            event_rx: Some(event_rx),
            resize_tx,
            resize_rx: Some(resize_rx),
        }
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
        let event_rx = self
            .event_rx
            .take()
            .ok_or_else(|| anyhow::anyhow!("capture already started (only one connection supported)"))?;

        let resize_rx = self
            .resize_rx
            .take()
            .ok_or_else(|| anyhow::anyhow!("resize channel already taken"))?;

        Ok(Box::new(LiveDisplayUpdates {
            event_rx,
            resize_rx,
            pending_cursor: None,
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

        tracing::info!(width, height, "Client requested display resize");

        let new_size = DesktopSize { width, height };
        if let Err(e) = self.resize_tx.try_send(new_size) {
            tracing::warn!("Failed to send resize request: {e}");
        }
    }
}

/// Display updates that receive live frames from the `PipeWire` capture
/// and handle dynamic resize events from the RDP client.
struct LiveDisplayUpdates {
    event_rx: mpsc::Receiver<CaptureEvent>,
    resize_rx: mpsc::Receiver<DesktopSize>,
    /// When a `FrameAndCursor` event arrives, we return the frame first
    /// and buffer the cursor update for the next call.
    pending_cursor: Option<CursorInfo>,
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for LiveDisplayUpdates {
    async fn next_update(&mut self) -> Result<Option<DisplayUpdate>> {
        // If we have a buffered cursor update from a previous FrameAndCursor,
        // return it immediately before reading more events.
        if let Some(cursor) = self.pending_cursor.take() {
            return Ok(Some(cursor_to_display_update(&cursor)));
        }

        // All branches are cancellation-safe (mpsc::Receiver::recv)
        tokio::select! {
            event = self.event_rx.recv() => {
                let Some(event) = event else {
                    return Ok(None);
                };

                match event {
                    CaptureEvent::Frame(mut frame) => {
                        frame.ensure_alpha_opaque();
                        let bitmap = frame_to_bitmap(frame)?;
                        Ok(Some(DisplayUpdate::Bitmap(bitmap)))
                    }
                    CaptureEvent::Cursor(cursor) => {
                        Ok(Some(cursor_to_display_update(&cursor)))
                    }
                    CaptureEvent::FrameAndCursor(mut frame, cursor) => {
                        // Return the frame now, buffer cursor for next call.
                        self.pending_cursor = Some(cursor);
                        frame.ensure_alpha_opaque();
                        let bitmap = frame_to_bitmap(frame)?;
                        Ok(Some(DisplayUpdate::Bitmap(bitmap)))
                    }
                }
            }
            size = self.resize_rx.recv() => {
                let Some(new_size) = size else {
                    // Resize channel closed - continue with frames only.
                    // This shouldn't normally happen.
                    return Ok(None);
                };
                tracing::info!(
                    width = new_size.width,
                    height = new_size.height,
                    "Emitting display resize"
                );
                Ok(Some(DisplayUpdate::Resize(new_size)))
            }
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
    server
}

/// Build an RDP server with live screen capture and input injection.
pub fn build_live_server(
    bind_addr: std::net::SocketAddr,
    tls: &TlsContext,
    auth: Option<&AuthCredentials>,
    display: LiveDisplay,
    input_handler: LiveInputHandler,
    cliprdr: Option<Box<dyn CliprdrServerFactory>>,
    sound: Option<Box<dyn SoundServerFactory>>,
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
