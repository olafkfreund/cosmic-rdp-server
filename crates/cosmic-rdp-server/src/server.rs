use std::num::{NonZeroU16, NonZeroUsize};

use anyhow::Result;
use bytes::Bytes;
use ironrdp_server::{
    BitmapUpdate, DesktopSize, DisplayUpdate, KeyboardEvent, MouseEvent, PixelFormat, RdpServer,
    RdpServerDisplay, RdpServerDisplayUpdates, RdpServerInputHandler,
};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

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

/// Build and return the RDP server, ready to accept connections.
pub fn build_server(bind_addr: std::net::SocketAddr, tls_acceptor: TlsAcceptor) -> RdpServer {
    RdpServer::builder()
        .with_addr(bind_addr)
        .with_tls(tls_acceptor)
        .with_input_handler(StaticInputHandler)
        .with_display_handler(StaticDisplay::default())
        .build()
}
