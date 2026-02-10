/// A rectangular region of damage (changed pixels).
#[derive(Debug, Clone, PartialEq)]
pub struct DamageRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl DamageRect {
    #[must_use] 
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Create a damage rect covering the full frame.
    #[must_use] 
    pub fn full_frame(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    #[must_use] 
    pub fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

/// Pixel format of captured frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// BGRA with 8 bits per channel (`PipeWire` `BGRx` with alpha = 0xFF).
    Bgra,
    /// RGBA with 8 bits per channel.
    Rgba,
}

impl PixelFormat {
    #[must_use] 
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Bgra | Self::Rgba => 4,
        }
    }
}

/// Cursor bitmap data (RGBA pixels).
#[derive(Debug, Clone)]
pub struct CursorBitmap {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Hotspot X coordinate.
    pub hot_x: u32,
    /// Hotspot Y coordinate.
    pub hot_y: u32,
    /// RGBA pixel data (4 bytes per pixel, top-to-bottom row order).
    pub data: Vec<u8>,
}

impl CursorBitmap {
    /// Expected data length for the given dimensions.
    #[must_use]
    pub fn expected_len(width: u32, height: u32) -> usize {
        (width as usize) * (height as usize) * 4
    }

    /// Validate that the bitmap data matches the declared dimensions.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.data.len() == Self::expected_len(self.width, self.height)
    }
}

/// Cursor position and optional shape information.
#[derive(Debug, Clone)]
pub struct CursorInfo {
    /// Cursor X position relative to the captured region.
    pub x: i32,
    /// Cursor Y position relative to the captured region.
    pub y: i32,
    /// Whether the cursor is visible.
    pub visible: bool,
    /// Cursor bitmap, if the shape changed since last update.
    pub bitmap: Option<CursorBitmap>,
}

/// A chunk of captured audio data.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Raw PCM audio data.
    pub data: Vec<u8>,
    /// Number of audio channels.
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Bits per sample (e.g. 16).
    pub bits_per_sample: u16,
    /// Monotonically increasing sequence number.
    pub sequence: u64,
}

/// Events produced by the capture pipeline.
#[derive(Debug, Clone)]
pub enum CaptureEvent {
    /// A new video frame is available.
    Frame(CapturedFrame),
    /// Cursor position or shape changed (no new frame).
    Cursor(CursorInfo),
    /// A new video frame and cursor update arrived together.
    FrameAndCursor(CapturedFrame, CursorInfo),
}

/// A single captured video frame.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// Raw pixel data (BGRA or RGBA, top-to-bottom row order).
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: PixelFormat,
    /// Row stride in bytes.
    pub stride: u32,
    /// Frame sequence number (monotonically increasing).
    pub sequence: u64,
    /// Damage regions, if available.
    /// `None` means no damage info (treat as full frame).
    /// Empty vec means no damage (frame identical to previous).
    pub damage: Option<Vec<DamageRect>>,
}

impl CapturedFrame {
    /// Convert `BGRx` data to BGRA by setting alpha to 0xFF.
    ///
    /// `PipeWire` typically delivers `BGRx` format where the 'x' padding byte
    /// is undefined. This ensures the alpha channel is fully opaque.
    pub fn ensure_alpha_opaque(&mut self) {
        if self.format == PixelFormat::Bgra {
            for chunk in self.data.chunks_exact_mut(4) {
                chunk[3] = 0xFF;
            }
        }
    }
}
