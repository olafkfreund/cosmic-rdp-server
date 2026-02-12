//! `PipeWire` audio capture for RDPSND forwarding.
//!
//! Captures desktop audio by connecting to the default audio sink's monitor
//! port via `PipeWire`. Runs on a dedicated OS thread with its own main loop,
//! sending [`AudioChunk`] samples to a tokio mpsc channel.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use pipewire as pw;
use pw::properties::properties;
use pw::stream::{Stream, StreamFlags, StreamState};
use tokio::sync::mpsc;

use crate::frame::AudioChunk;

/// Handle to a running `PipeWire` audio capture stream.
///
/// Dropping this stops the audio capture thread.
pub struct PwAudioStream {
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for PwAudioStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PwAudioStream")
            .field("running", &self.running.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl PwAudioStream {
    /// Start capturing audio from the default sink monitor.
    ///
    /// # Errors
    ///
    /// Returns `AudioCaptureError` if the thread cannot be spawned.
    pub fn start(
        channels: u16,
        sample_rate: u32,
        channel_capacity: usize,
    ) -> Result<(Self, mpsc::Receiver<AudioChunk>), AudioCaptureError> {
        let (tx, rx) = mpsc::channel(channel_capacity);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);

        let thread = std::thread::Builder::new()
            .name("pw-audio".into())
            .spawn(move || {
                if let Err(e) =
                    run_audio_loop(channels, sample_rate, tx, running_clone)
                {
                    tracing::error!("PipeWire audio thread exited with error: {e}");
                }
            })
            .map_err(AudioCaptureError::SpawnThread)?;

        Ok((
            Self {
                running,
                thread: Some(thread),
            },
            rx,
        ))
    }

    /// Stop the audio capture and join the thread.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PwAudioStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run the `PipeWire` audio main loop on a dedicated thread.
#[allow(clippy::needless_pass_by_value)]
fn run_audio_loop(
    channels: u16,
    sample_rate: u32,
    audio_tx: mpsc::Sender<AudioChunk>,
    running: Arc<AtomicBool>,
) -> Result<(), AudioCaptureError> {
    pw::init();

    let mainloop = pw::main_loop::MainLoop::new(None).map_err(|_| AudioCaptureError::MainLoop)?;
    let context =
        pw::context::Context::new(&mainloop).map_err(|_| AudioCaptureError::Context)?;
    let core = context
        .connect(None)
        .map_err(|_| AudioCaptureError::Connect)?;

    let stream = Stream::new(
        &core,
        "cosmic-ext-rdp-audio",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::STREAM_CAPTURE_SINK => "true",
        },
    )
    .map_err(|_| AudioCaptureError::CreateStream)?;

    let seq = Arc::new(AtomicU64::new(0));
    let ch = channels;
    let rate = sample_rate;

    let _listener = stream
        .add_local_listener_with_user_data(audio_tx)
        .state_changed(|_stream, _tx, old, new| {
            tracing::debug!("PipeWire audio stream state: {old:?} -> {new:?}");
            if new == StreamState::Error(String::new()) {
                tracing::error!("PipeWire audio stream entered error state");
            }
        })
        .process(move |stream_ref, tx| {
            process_audio(stream_ref, tx, &seq, ch, rate);
        })
        .register()
        .map_err(|_| AudioCaptureError::RegisterListener)?;

    // Build SPA audio info params for negotiation.
    // S16LE format, specified channels and sample rate.
    let audio_info = pw::spa::param::audio::AudioInfoRaw::new();
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: pw::spa::param::ParamType::EnumFormat.as_raw(),
            properties: audio_info.into(),
        }),
    )
    .map_err(|_| AudioCaptureError::CreateStream)?
    .0
    .into_inner();

    let mut params = [pw::spa::pod::Pod::from_bytes(&values).expect("valid pod")];

    stream
        .connect(
            pw::spa::utils::Direction::Input,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|_| AudioCaptureError::StreamConnect)?;

    tracing::info!(channels, sample_rate, "PipeWire audio stream connected");

    while running.load(Ordering::SeqCst) {
        mainloop.loop_().iterate(std::time::Duration::from_millis(50));
    }

    tracing::info!("PipeWire audio main loop exiting");
    Ok(())
}

/// Process a single audio buffer from the `PipeWire` stream.
fn process_audio(
    stream: &pw::stream::StreamRef,
    tx: &mut mpsc::Sender<AudioChunk>,
    seq: &AtomicU64,
    channels: u16,
    sample_rate: u32,
) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };

    let datas = buffer.datas_mut();
    if datas.is_empty() {
        return;
    }

    let data = &mut datas[0];
    let chunk = data.chunk();
    let size = chunk.size() as usize;

    let Some(slice) = data.data() else {
        return;
    };

    if size == 0 || size > slice.len() {
        return;
    }

    let audio_data = slice[..size].to_vec();
    let sequence = seq.fetch_add(1, Ordering::Relaxed);

    let chunk = AudioChunk {
        data: audio_data,
        channels,
        sample_rate,
        bits_per_sample: 16,
        sequence,
    };

    if tx.try_send(chunk).is_err() {
        tracing::trace!("Audio channel full, dropping chunk {sequence}");
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AudioCaptureError {
    #[error("failed to create PipeWire MainLoop")]
    MainLoop,

    #[error("failed to create PipeWire Context")]
    Context,

    #[error("failed to connect to PipeWire")]
    Connect,

    #[error("failed to create PipeWire audio Stream")]
    CreateStream,

    #[error("failed to register audio stream listener")]
    RegisterListener,

    #[error("failed to connect audio stream")]
    StreamConnect,

    #[error("failed to spawn PipeWire audio thread")]
    SpawnThread(#[source] std::io::Error),
}
