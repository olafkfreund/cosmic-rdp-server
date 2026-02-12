use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use pipewire as pw;
use pw::properties::properties;
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::Pod;
use pw::stream::{Stream, StreamFlags, StreamState};
use tokio::sync::mpsc;

use crate::frame::{CaptureEvent, CapturedFrame, PixelFormat};

/// Handle to a running `PipeWire` capture stream.
///
/// The stream runs on a dedicated OS thread with its own `PipeWire` `MainLoop`.
/// Frames are delivered via a tokio mpsc channel.
pub struct PwStream {
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PwStream {
    /// Start capturing from the given `PipeWire` node using the portal's fd.
    ///
    /// Returns a `PwStream` handle and a receiver for captured frames.
    ///
    /// # Errors
    ///
    /// Returns `PwError` if the `PipeWire` thread cannot be spawned.
    pub fn start(
        pipewire_fd: OwnedFd,
        node_id: u32,
        channel_capacity: usize,
        swap_colors: bool,
    ) -> Result<(Self, mpsc::Receiver<CaptureEvent>), PwError> {
        let (tx, rx) = mpsc::channel(channel_capacity);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);

        let thread = std::thread::Builder::new()
            .name("pw-capture".into())
            .spawn(move || {
                if let Err(e) =
                    run_pipewire_loop(pipewire_fd, node_id, tx, running_clone, swap_colors)
                {
                    tracing::error!("PipeWire thread exited with error: {e}");
                }
            })
            .map_err(PwError::SpawnThread)?;

        Ok((
            Self {
                running,
                thread: Some(thread),
            },
            rx,
        ))
    }

    /// Stop the `PipeWire` stream and join the thread.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PwStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run the `PipeWire` main loop on a dedicated thread.
#[allow(clippy::needless_pass_by_value)] // Arc is moved from a thread spawn closure
fn run_pipewire_loop(
    pipewire_fd: OwnedFd,
    node_id: u32,
    frame_tx: mpsc::Sender<CaptureEvent>,
    running: Arc<AtomicBool>,
    swap_colors: bool,
) -> Result<(), PwError> {
    pw::init();

    let mainloop = pw::main_loop::MainLoop::new(None).map_err(|_| PwError::MainLoop)?;
    let context = pw::context::Context::new(&mainloop).map_err(|_| PwError::Context)?;
    let core = context
        .connect_fd(pipewire_fd, None)
        .map_err(|_| PwError::ConnectFd)?;

    let stream = Stream::new(
        &core,
        "cosmic-ext-rdp-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|_| PwError::CreateStream)?;

    let seq = Arc::new(AtomicU64::new(0));
    // Track the negotiated pixel format (SPA_VIDEO_FORMAT_* value).
    // Default to BGRx since that's our preferred format.
    let negotiated_format = Arc::new(AtomicU32::new(
        pw::spa::param::video::VideoFormat::BGRx.as_raw(),
    ));
    let negotiated_format_cb = Arc::clone(&negotiated_format);

    let _listener = stream
        .add_local_listener_with_user_data(frame_tx)
        .state_changed(|_stream, _tx, old, new| {
            tracing::debug!("PipeWire stream state: {old:?} -> {new:?}");
            if new == StreamState::Error(String::new()) {
                tracing::error!("PipeWire stream entered error state");
            }
        })
        .param_changed(move |_stream, _tx, id, pod| {
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            if let Some(pod) = pod {
                // Parse the format pod to extract the negotiated video format.
                if let Ok((_, pw::spa::pod::Value::Object(obj))) = pw::spa::pod::deserialize::PodDeserializer::deserialize_any_from(pod.as_bytes()) {
                    for prop in &obj.properties {
                        if prop.key == pw::spa::param::format::FormatProperties::VideoFormat.as_raw() {
                            if let pw::spa::pod::Value::Id(fmt_id) = prop.value {
                                negotiated_format_cb.store(fmt_id.0, Ordering::SeqCst);
                                tracing::info!(format_id = fmt_id.0, "PipeWire negotiated video format");
                            }
                        }
                    }
                }
            }
        })
        .process(move |stream_ref, tx| {
            process_frame(stream_ref, tx, &seq, &negotiated_format, swap_colors);
        })
        .register()
        .map_err(|_| PwError::RegisterListener)?;

    // Request BGRx/BGRA SHM format explicitly. Without format params,
    // PipeWire may negotiate DMA-BUF which yields black frames when
    // MAP_BUFFERS maps GPU memory that hasn't been synced to CPU.
    let format_pod = build_video_format_pod();
    let mut params = [Pod::from_bytes(&format_pod).expect("valid format pod")];

    stream
        .connect(
            pw::spa::utils::Direction::Input,
            Some(node_id),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|_| PwError::StreamConnect)?;

    tracing::info!(node_id, "PipeWire stream connected, entering main loop");

    while running.load(Ordering::SeqCst) {
        mainloop.loop_().iterate(std::time::Duration::from_millis(50));
    }

    tracing::info!("PipeWire main loop exiting");
    Ok(())
}

/// Build a SPA format pod requesting BGRx/BGRA raw video in SHM.
///
/// This tells `PipeWire` to prefer shared-memory buffers with CPU-readable
/// pixel data instead of DMA-BUF handles that may yield black frames.
fn build_video_format_pod() -> Vec<u8> {
    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::BGRA,
            pw::spa::param::video::VideoFormat::RGBx,
            pw::spa::param::video::VideoFormat::RGBA
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle { width: 1920, height: 1080 },
            pw::spa::utils::Rectangle { width: 1, height: 1 },
            pw::spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 30, denom: 1 },
            pw::spa::utils::Fraction { num: 1, denom: 1 },
            pw::spa::utils::Fraction { num: 120, denom: 1 }
        ),
    );

    PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .expect("format pod serialization")
    .0
    .into_inner()
}

/// Process a single frame from the `PipeWire` stream.
///
/// Uses the raw `PipeWire` buffer API to access SPA metadata (damage rects,
/// cursor info) that the safe `dequeue_buffer()` wrapper does not expose.
fn process_frame(
    stream: &pw::stream::StreamRef,
    tx: &mut mpsc::Sender<CaptureEvent>,
    seq: &AtomicU64,
    negotiated_format: &AtomicU32,
    swap_colors: bool,
) {
    // Dequeue buffer using raw API for SPA metadata access.
    // Safety: stream is valid within the process callback.
    let raw_pw_buf = unsafe { stream.dequeue_raw_buffer() };
    if raw_pw_buf.is_null() {
        return;
    }

    // Safety: raw_pw_buf is valid while dequeued.
    let spa_buf = unsafe { (*raw_pw_buf).buffer };
    if spa_buf.is_null() {
        // Safety: returning the buffer we dequeued from this stream.
        unsafe { stream.queue_raw_buffer(raw_pw_buf) };
        return;
    }

    // Extract SPA metadata before reading frame pixel data.
    let damage = unsafe { crate::spa_meta::extract_damage(spa_buf) };
    let cursor = unsafe { crate::spa_meta::extract_cursor(spa_buf) };

    // Access frame data through the raw spa_data array.
    let (n_datas, datas_ptr) = unsafe { ((*spa_buf).n_datas, (*spa_buf).datas) };
    if n_datas == 0 || datas_ptr.is_null() {
        unsafe { stream.queue_raw_buffer(raw_pw_buf) };
        return;
    }

    // Safety: n_datas > 0 and datas_ptr is valid; Data is #[repr(transparent)].
    let data: &mut pw::spa::buffer::Data =
        unsafe { &mut *datas_ptr.cast::<pw::spa::buffer::Data>() };

    // Read chunk metadata before taking the mutable data borrow.
    let chunk = data.chunk();
    #[allow(clippy::cast_sign_loss)] // negative stride is invalid, treated as zero below
    let stride = chunk.stride() as u32;
    let offset = chunk.offset() as usize;
    let size = chunk.size() as usize;

    let Some(slice) = data.data() else {
        unsafe { stream.queue_raw_buffer(raw_pw_buf) };
        return;
    };

    if size == 0 || stride == 0 {
        unsafe { stream.queue_raw_buffer(raw_pw_buf) };
        return;
    }

    // Infer dimensions from stride and size.
    // PipeWire BGRx/BGRA is 4 bytes per pixel.
    let bpp = 4u32;
    let width = stride / bpp;
    #[allow(clippy::cast_possible_truncation)] // frame size always fits in u32
    let height = if stride > 0 { (size as u32) / stride } else { 0 };

    if width == 0 || height == 0 {
        unsafe { stream.queue_raw_buffer(raw_pw_buf) };
        return;
    }

    let end = offset + size;
    if end > slice.len() {
        tracing::warn!(
            offset,
            size,
            slice_len = slice.len(),
            "Buffer slice out of bounds"
        );
        unsafe { stream.queue_raw_buffer(raw_pw_buf) };
        return;
    }

    // Copy pixel data before returning the buffer to PipeWire.
    let mut frame_data = slice[offset..end].to_vec();
    let sequence = seq.fetch_add(1, Ordering::Relaxed);

    // Safety: we've finished reading from the buffer, return it to PipeWire.
    unsafe { stream.queue_raw_buffer(raw_pw_buf) };

    // Check if PipeWire negotiated an RGB-order format (RGBx or RGBA).
    // The RDP server expects BGRA, so swap R and B channels if needed.
    let fmt = negotiated_format.load(Ordering::Relaxed);

    // Log raw pixel bytes on the first frame to diagnose color channel order.
    if sequence == 0 && frame_data.len() >= 12 {
        tracing::info!(
            spa_format_id = fmt,
            bgrx_id = pw::spa::param::video::VideoFormat::BGRx.as_raw(),
            rgbx_id = pw::spa::param::video::VideoFormat::RGBx.as_raw(),
            raw_pixel_0 = format_args!("[{:#04x},{:#04x},{:#04x},{:#04x}]",
                frame_data[0], frame_data[1], frame_data[2], frame_data[3]),
            raw_pixel_1 = format_args!("[{:#04x},{:#04x},{:#04x},{:#04x}]",
                frame_data[4], frame_data[5], frame_data[6], frame_data[7]),
            "PipeWire first frame: raw pixels BEFORE any swap"
        );
    }

    let is_rgb_order = fmt == pw::spa::param::video::VideoFormat::RGBx.as_raw()
        || fmt == pw::spa::param::video::VideoFormat::RGBA.as_raw();
    if is_rgb_order ^ swap_colors {
        for pixel in frame_data.chunks_exact_mut(4) {
            pixel.swap(0, 2); // R,G,B,A -> B,G,R,A  (or force swap when override set)
        }
    }

    let frame = CapturedFrame {
        data: frame_data,
        width,
        height,
        format: PixelFormat::Bgra,
        stride,
        sequence,
        damage,
    };

    // Non-blocking send. Drop frame if channel is full to avoid backpressure.
    let event = if let Some(cursor_info) = cursor {
        CaptureEvent::FrameAndCursor(frame, cursor_info)
    } else {
        CaptureEvent::Frame(frame)
    };
    if tx.try_send(event).is_err() {
        tracing::trace!("Frame channel full, dropping frame {sequence}");
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PwError {
    #[error("failed to create PipeWire MainLoop")]
    MainLoop,

    #[error("failed to create PipeWire Context")]
    Context,

    #[error("failed to connect to PipeWire via portal fd")]
    ConnectFd,

    #[error("failed to create PipeWire Stream")]
    CreateStream,

    #[error("failed to register stream listener")]
    RegisterListener,

    #[error("failed to connect stream to node")]
    StreamConnect,

    #[error("failed to spawn PipeWire thread")]
    SpawnThread(#[source] std::io::Error),
}
