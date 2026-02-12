//! RDPSND audio forwarding backend.
//!
//! Captures desktop audio via `PipeWire` and forwards it to the RDP client
//! over the RDPSND virtual channel.

use ironrdp_rdpsnd::pdu::{AudioFormat, ClientAudioFormatPdu, WaveFormat};
use ironrdp_server::{
    RdpsndServerHandler, RdpsndServerMessage, ServerEvent, ServerEventSender, SoundServerFactory,
};
use rdp_capture::{AudioChunk, PwAudioStream};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Handler (one per RDP connection)
// ---------------------------------------------------------------------------

/// RDPSND handler that captures audio from `PipeWire` and sends wave data
/// to the RDP client.
#[derive(Debug)]
pub struct PipeWireAudioHandler {
    formats: Vec<AudioFormat>,
    channels: u16,
    sample_rate: u32,
    event_tx: mpsc::UnboundedSender<ServerEvent>,
    audio_stream: Option<PwAudioStream>,
    pump_abort: Option<tokio::sync::oneshot::Sender<()>>,
}

impl PipeWireAudioHandler {
    fn new(
        channels: u16,
        sample_rate: u32,
        event_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> Self {
        let block_align = channels * 2; // 16-bit samples
        let avg_bytes_per_sec = u32::from(block_align) * sample_rate;

        let formats = vec![AudioFormat {
            format: WaveFormat::PCM,
            n_channels: channels,
            n_samples_per_sec: sample_rate,
            n_avg_bytes_per_sec: avg_bytes_per_sec,
            n_block_align: block_align,
            bits_per_sample: 16,
            data: None,
        }];

        Self {
            formats,
            channels,
            sample_rate,
            event_tx,
            audio_stream: None,
            pump_abort: None,
        }
    }

    /// Forward audio chunks from `PipeWire` to the RDP RDPSND channel.
    fn start_pump(
        &self,
        audio_rx: mpsc::Receiver<AudioChunk>,
    ) -> tokio::sync::oneshot::Sender<()> {
        let event_tx = self.event_tx.clone();
        let (abort_tx, mut abort_rx) = tokio::sync::oneshot::channel();

        // Use a tokio runtime handle. The handler runs on the server's
        // async context, so `Handle::current()` is available.
        let rt = tokio::runtime::Handle::current();
        rt.spawn(async move {
            let mut audio_rx = audio_rx;
            loop {
                tokio::select! {
                    chunk = audio_rx.recv() => {
                        let Some(chunk) = chunk else {
                            tracing::debug!("Audio capture channel closed");
                            break;
                        };
                        // Timestamp in milliseconds (approximate from sequence).
                        #[allow(clippy::cast_possible_truncation)]
                        let ts = (chunk.sequence * 1000 / u64::from(chunk.sample_rate)) as u32;
                        let msg = RdpsndServerMessage::Wave(chunk.data, ts);
                        if event_tx.send(ServerEvent::Rdpsnd(msg)).is_err() {
                            tracing::debug!("Sound event channel closed");
                            break;
                        }
                    }
                    _ = &mut abort_rx => {
                        tracing::debug!("Audio pump aborted");
                        break;
                    }
                }
            }
        });

        abort_tx
    }
}

impl RdpsndServerHandler for PipeWireAudioHandler {
    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn start(&mut self, _client_format: &ClientAudioFormatPdu) -> Option<u16> {
        tracing::info!(
            channels = self.channels,
            sample_rate = self.sample_rate,
            "Starting audio capture for RDPSND"
        );

        match PwAudioStream::start(self.channels, self.sample_rate, 32) {
            Ok((stream, audio_rx)) => {
                let abort = self.start_pump(audio_rx);
                self.audio_stream = Some(stream);
                self.pump_abort = Some(abort);
                // Return format index 0 (our PCM format).
                Some(0)
            }
            Err(e) => {
                tracing::warn!("Failed to start PipeWire audio capture: {e}");
                None
            }
        }
    }

    fn stop(&mut self) {
        tracing::info!("Stopping audio capture");
        // Abort the pump task first.
        if let Some(abort) = self.pump_abort.take() {
            let _ = abort.send(());
        }
        // Then stop the PipeWire stream.
        if let Some(mut stream) = self.audio_stream.take() {
            stream.stop();
        }
    }
}

// ---------------------------------------------------------------------------
// Factory (shared across connections)
// ---------------------------------------------------------------------------

/// Factory that creates [`PipeWireAudioHandler`] instances for each RDP
/// connection.
#[derive(Debug)]
pub struct PipeWireAudioFactory {
    channels: u16,
    sample_rate: u32,
    event_tx: Option<mpsc::UnboundedSender<ServerEvent>>,
}

impl PipeWireAudioFactory {
    pub fn new(channels: u16, sample_rate: u32) -> Self {
        Self {
            channels,
            sample_rate,
            event_tx: None,
        }
    }
}

impl ServerEventSender for PipeWireAudioFactory {
    fn set_sender(&mut self, sender: mpsc::UnboundedSender<ServerEvent>) {
        self.event_tx = Some(sender);
    }
}

impl SoundServerFactory for PipeWireAudioFactory {
    fn build_backend(&self) -> Box<dyn RdpsndServerHandler> {
        let tx = self
            .event_tx
            .clone()
            .expect("set_sender must be called before build_backend");
        Box::new(PipeWireAudioHandler::new(
            self.channels,
            self.sample_rate,
            tx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_format_valid() {
        let handler = PipeWireAudioHandler::new(
            2,
            44100,
            mpsc::unbounded_channel().0,
        );
        let formats = handler.get_formats();
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0].format, WaveFormat::PCM);
        assert_eq!(formats[0].n_channels, 2);
        assert_eq!(formats[0].n_samples_per_sec, 44100);
        assert_eq!(formats[0].bits_per_sample, 16);
        assert_eq!(formats[0].n_block_align, 4); // 2 channels * 2 bytes
        assert_eq!(formats[0].n_avg_bytes_per_sec, 176_400); // 44100 * 4
    }
}
