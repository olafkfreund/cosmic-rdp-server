//! Multi-monitor frame compositor.
//!
//! Merges per-monitor capture streams into a single virtual desktop frame.
//! When only one monitor is present, acts as a zero-overhead passthrough.

use tokio::sync::mpsc;

use crate::frame::{CaptureEvent, CapturedFrame, CursorInfo, DamageRect, PixelFormat};

/// Information about a single captured monitor.
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    /// `PipeWire` node ID.
    pub node_id: u32,
    /// Monitor width in pixels.
    pub width: u16,
    /// Monitor height in pixels.
    pub height: u16,
    /// X offset in the virtual desktop.
    pub x: i32,
    /// Y offset in the virtual desktop.
    pub y: i32,
}

/// Compute the bounding box of a set of monitors.
///
/// Returns `(width, height)` of the virtual desktop that encompasses
/// all monitors at their respective positions.
#[must_use]
pub fn bounding_box(monitors: &[MonitorInfo]) -> (u16, u16) {
    if monitors.is_empty() {
        return (0, 0);
    }

    let mut max_x: i32 = 0;
    let mut max_y: i32 = 0;

    for m in monitors {
        let right = m.x + i32::from(m.width);
        let bottom = m.y + i32::from(m.height);
        max_x = max_x.max(right);
        max_y = max_y.max(bottom);
    }

    // Clamp to u16 range.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let width = max_x.max(0).min(i32::from(u16::MAX)) as u16;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let height = max_y.max(0).min(i32::from(u16::MAX)) as u16;

    (width, height)
}

/// A per-monitor input channel with its offset in the virtual desktop.
struct MonitorInput {
    rx: mpsc::Receiver<CaptureEvent>,
    x_offset: i32,
    y_offset: i32,
}

/// Compositor that merges multiple monitor streams into a single virtual
/// desktop.
///
/// Each monitor's frames are blitted at the correct offset into a canvas
/// that represents the full virtual desktop. Cursor events have their
/// positions adjusted by the monitor offset.
pub struct FrameCompositor {
    monitors: Vec<MonitorInput>,
    canvas_width: u16,
    canvas_height: u16,
    output_tx: mpsc::Sender<CaptureEvent>,
    sequence: u64,
}

impl FrameCompositor {
    /// Create a new compositor.
    ///
    /// Returns the compositor and a receiver for the composed output events.
    #[must_use]
    pub fn new(
        monitor_infos: &[MonitorInfo],
        monitor_rxs: Vec<mpsc::Receiver<CaptureEvent>>,
        output_capacity: usize,
    ) -> (Self, mpsc::Receiver<CaptureEvent>) {
        let (output_tx, output_rx) = mpsc::channel(output_capacity);
        let (canvas_width, canvas_height) = bounding_box(monitor_infos);

        let monitors: Vec<MonitorInput> = monitor_infos
            .iter()
            .zip(monitor_rxs)
            .map(|(info, rx)| MonitorInput {
                rx,
                x_offset: info.x,
                y_offset: info.y,
            })
            .collect();

        (
            Self {
                monitors,
                canvas_width,
                canvas_height,
                output_tx,
                sequence: 0,
            },
            output_rx,
        )
    }

    /// Run the compositor loop, selecting across all monitor inputs.
    ///
    /// This should be spawned on a tokio task. Exits when all input channels
    /// close or the output channel is dropped.
    pub async fn run(mut self) {
        // Store the latest frame from each monitor for compositing.
        let num_monitors = self.monitors.len();
        let mut latest_frames: Vec<Option<CapturedFrame>> = vec![None; num_monitors];

        loop {
            // We use a polling approach: try_recv from each monitor,
            // then compose if any new frame arrived.
            let mut any_new = false;

            for (i, monitor) in self.monitors.iter_mut().enumerate() {
                match monitor.rx.try_recv() {
                    Ok(event) => {
                        match event {
                            CaptureEvent::Frame(frame) => {
                                latest_frames[i] = Some(frame);
                                any_new = true;
                            }
                            CaptureEvent::FrameAndCursor(frame, cursor) => {
                                latest_frames[i] = Some(frame);
                                any_new = true;
                                // Forward cursor with adjusted position.
                                let adjusted = adjust_cursor(
                                    &cursor,
                                    monitor.x_offset,
                                    monitor.y_offset,
                                );
                                let _ = self.output_tx.try_send(CaptureEvent::Cursor(adjusted));
                            }
                            CaptureEvent::Cursor(cursor) => {
                                let adjusted = adjust_cursor(
                                    &cursor,
                                    monitor.x_offset,
                                    monitor.y_offset,
                                );
                                let _ = self.output_tx.try_send(CaptureEvent::Cursor(adjusted));
                            }
                        }
                    }
                    Err(
                        mpsc::error::TryRecvError::Empty
                        | mpsc::error::TryRecvError::Disconnected,
                    ) => {}
                }
            }

            if any_new {
                // Compose all latest frames into a single canvas.
                if let Some(composed) = self.compose(&latest_frames) {
                    if self.output_tx.try_send(CaptureEvent::Frame(composed)).is_err() {
                        tracing::trace!("Compositor output channel full");
                    }
                }
            }

            // Sleep briefly to avoid busy-waiting.
            tokio::time::sleep(std::time::Duration::from_millis(8)).await;

            // Check if output is still open.
            if self.output_tx.is_closed() {
                break;
            }
        }
    }

    /// Blit all monitor frames onto a single BGRA canvas.
    fn compose(&mut self, frames: &[Option<CapturedFrame>]) -> Option<CapturedFrame> {
        let w = usize::from(self.canvas_width);
        let h = usize::from(self.canvas_height);
        let bpp = 4usize;
        let canvas_stride = w * bpp;
        let mut canvas = vec![0u8; canvas_stride * h];

        let mut any_frame = false;

        for (i, monitor) in self.monitors.iter().enumerate() {
            let Some(frame) = frames[i].as_ref() else {
                continue;
            };
            any_frame = true;
            blit_frame(
                &mut canvas,
                canvas_stride,
                frame,
                monitor.x_offset,
                monitor.y_offset,
                self.canvas_width,
                self.canvas_height,
            );
        }

        if !any_frame {
            return None;
        }

        self.sequence += 1;

        #[allow(clippy::cast_possible_truncation)]
        Some(CapturedFrame {
            data: canvas,
            width: u32::from(self.canvas_width),
            height: u32::from(self.canvas_height),
            format: PixelFormat::Bgra,
            stride: canvas_stride as u32,
            sequence: self.sequence,
            damage: Some(vec![DamageRect::full_frame(
                u32::from(self.canvas_width),
                u32::from(self.canvas_height),
            )]),
        })
    }
}

/// Blit a single frame onto the canvas at the given offset.
fn blit_frame(
    canvas: &mut [u8],
    canvas_stride: usize,
    frame: &CapturedFrame,
    x_offset: i32,
    y_offset: i32,
    canvas_width: u16,
    canvas_height: u16,
) {
    let bpp = 4usize;
    let frame_stride = frame.stride as usize;

    for row in 0..frame.height {
        #[allow(clippy::cast_possible_wrap)]
        let dst_y = y_offset + row as i32;
        if dst_y < 0 || dst_y >= i32::from(canvas_height) {
            continue;
        }

        #[allow(clippy::cast_possible_wrap)]
        let src_start = (row as usize) * frame_stride;
        let src_end = src_start + (frame.width as usize) * bpp;
        if src_end > frame.data.len() {
            continue;
        }

        // Determine the visible horizontal range.
        let dst_x_start = x_offset.max(0);
        #[allow(clippy::cast_possible_wrap)]
        let dst_x_end = (x_offset + frame.width as i32).min(i32::from(canvas_width));
        if dst_x_start >= dst_x_end {
            continue;
        }

        #[allow(clippy::cast_sign_loss)]
        let src_skip = (dst_x_start - x_offset) as usize;
        #[allow(clippy::cast_sign_loss)]
        let copy_pixels = (dst_x_end - dst_x_start) as usize;

        let src_offset = src_start + src_skip * bpp;
        #[allow(clippy::cast_sign_loss)]
        let dst_offset = (dst_y as usize) * canvas_stride + (dst_x_start as usize) * bpp;

        let src_slice = &frame.data[src_offset..src_offset + copy_pixels * bpp];
        let dst_slice = &mut canvas[dst_offset..dst_offset + copy_pixels * bpp];
        dst_slice.copy_from_slice(src_slice);
    }
}

/// Adjust cursor position by monitor offset.
fn adjust_cursor(cursor: &CursorInfo, x_offset: i32, y_offset: i32) -> CursorInfo {
    CursorInfo {
        x: cursor.x + x_offset,
        y: cursor.y + y_offset,
        visible: cursor.visible,
        bitmap: cursor.bitmap.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_box_empty() {
        assert_eq!(bounding_box(&[]), (0, 0));
    }

    #[test]
    fn bounding_box_single() {
        let monitors = vec![MonitorInfo {
            node_id: 1,
            width: 1920,
            height: 1080,
            x: 0,
            y: 0,
        }];
        assert_eq!(bounding_box(&monitors), (1920, 1080));
    }

    #[test]
    fn bounding_box_dual_side_by_side() {
        let monitors = vec![
            MonitorInfo {
                node_id: 1,
                width: 1920,
                height: 1080,
                x: 0,
                y: 0,
            },
            MonitorInfo {
                node_id: 2,
                width: 1920,
                height: 1080,
                x: 1920,
                y: 0,
            },
        ];
        assert_eq!(bounding_box(&monitors), (3840, 1080));
    }

    #[test]
    fn bounding_box_stacked() {
        let monitors = vec![
            MonitorInfo {
                node_id: 1,
                width: 1920,
                height: 1080,
                x: 0,
                y: 0,
            },
            MonitorInfo {
                node_id: 2,
                width: 1920,
                height: 1080,
                x: 0,
                y: 1080,
            },
        ];
        assert_eq!(bounding_box(&monitors), (1920, 2160));
    }

    #[test]
    fn blit_simple() {
        let canvas_w = 4u16;
        let canvas_h = 4u16;
        let bpp = 4usize;
        let canvas_stride = usize::from(canvas_w) * bpp;
        let mut canvas = vec![0u8; canvas_stride * usize::from(canvas_h)];

        // 2x2 red frame at offset (1, 1)
        let frame = CapturedFrame {
            data: vec![0xFF; 2 * 2 * bpp],
            width: 2,
            height: 2,
            format: PixelFormat::Bgra,
            #[allow(clippy::cast_possible_truncation)]
            stride: (2 * bpp) as u32,
            sequence: 0,
            damage: None,
        };

        blit_frame(&mut canvas, canvas_stride, &frame, 1, 1, canvas_w, canvas_h);

        // Check that pixel (1,1) is filled.
        let offset = canvas_stride + bpp;
        assert_eq!(canvas[offset], 0xFF);

        // Check that pixel (0,0) is still zero.
        assert_eq!(canvas[0], 0);
    }
}
