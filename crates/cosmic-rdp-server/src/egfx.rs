//! EGFX/H.264 frame delivery integration.
//!
//! Bridges the `ironrdp-egfx` Graphics Pipeline Extension with our
//! `GstEncoder` H.264 pipeline, enabling 10-50x bandwidth reduction
//! over raw bitmap delivery.
//!
//! # Architecture
//!
//! Three components share state via `SharedEgfx` (`Arc<Mutex<EgfxInner>>`):
//!
//! - [`EgfxBridge`] – implements [`DvcProcessor`] and sits inside the
//!   DRDYNVC static virtual channel. Handles client↔server EGFX messages
//!   (capability negotiation, frame acks). On detecting readiness, auto-creates
//!   the surface and maps it to the output.
//!
//! - [`EgfxController`] – public handle used by `LiveDisplayUpdates` to
//!   check readiness, send H.264 frames, and obtain the DVC channel ID.
//!
//! - [`EgfxEventSetter`] – lightweight handle used after `RdpServer`
//!   construction to inject the server event sender into shared state.

use std::sync::{Arc, Mutex};

use ironrdp_core::{encode_vec, impl_as_any, Encode, WriteCursor};
use ironrdp_dvc::{DvcEncode, DvcMessage, DvcProcessor, DvcProcessorFactory, DvcServerProcessor};
use ironrdp_egfx::pdu::{Avc420Region, CapabilitiesAdvertisePdu, CapabilitySet};
use ironrdp_egfx::server::{GraphicsPipelineHandler, GraphicsPipelineServer};
use ironrdp_pdu::PduResult;
use ironrdp_server::ServerEvent;
use tokio::sync::mpsc;

/// H.264 quantization parameter for EGFX AVC420 regions.
/// Lower = better quality (18-23 is typical for RDP).
const EGFX_QP: u8 = 22;

/// Shared inner state between bridge, handler, and controller.
struct EgfxInner {
    server: GraphicsPipelineServer,
    ready: bool,
    surface_id: Option<u16>,
    dvc_channel_id: Option<u32>,
    supports_avc420: bool,
    width: u16,
    height: u16,
    event_tx: Option<mpsc::UnboundedSender<ServerEvent>>,
    /// Set `true` after `resize()` so the encoder forces an IDR keyframe
    /// on the next frame, ensuring the client can decode immediately.
    needs_keyframe: bool,
}

/// Thread-safe shared EGFX state.
type SharedEgfx = Arc<Mutex<EgfxInner>>;

/// Lock the shared state, logging a warning if the mutex was poisoned.
fn lock_shared(shared: &SharedEgfx) -> std::sync::MutexGuard<'_, EgfxInner> {
    shared.lock().unwrap_or_else(|e| {
        tracing::warn!("EGFX: mutex was poisoned, recovering (possible inconsistency)");
        e.into_inner()
    })
}

// --------------- ZGFX compression wrapper ---------------

/// ZGFX-wrapped DVC message (uncompressed single segment).
///
/// Per MS-RDPEGFX, all EGFX messages over the DVC channel MUST be wrapped
/// in ZGFX (RDP8 Bulk Compression). This sends an uncompressed segment:
/// `[0xE0 (single segment), 0x04 (uncompressed type), raw PDU bytes]`.
struct ZgfxWrapped {
    data: Vec<u8>,
}

impl Encode for ZgfxWrapped {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> ironrdp_core::EncodeResult<()> {
        dst.write_slice(&self.data);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "ZgfxWrapped"
    }

    fn size(&self) -> usize {
        self.data.len()
    }
}

// SAFETY: ZgfxWrapped only contains a Vec<u8> which is Send.
impl DvcEncode for ZgfxWrapped {}

/// Wrap each outgoing EGFX DVC message in a ZGFX uncompressed segment.
///
/// `FreeRDP` calls `zgfx_decompress()` on every incoming EGFX payload.
/// Without this wrapper, every frame is rejected with
/// `zgfx_decompress failure! status: -1`.
fn zgfx_wrap_messages(messages: Vec<DvcMessage>) -> Vec<DvcMessage> {
    messages
        .into_iter()
        .map(|msg| {
            let raw_size = msg.size();
            let raw = encode_vec(msg.as_ref()).unwrap_or_default();
            tracing::trace!(
                raw_size,
                encoded_len = raw.len(),
                first_bytes = ?&raw[..raw.len().min(32)],
                "EGFX ZGFX: encoding inner DvcMessage"
            );
            let mut data = Vec::with_capacity(2 + raw.len());
            data.push(0xE0); // ZGFX single segment descriptor
            data.push(0x04); // RDP8 uncompressed type
            data.extend_from_slice(&raw);
            tracing::trace!(
                total_len = data.len(),
                first_bytes = ?&data[..data.len().min(32)],
                "EGFX ZGFX: wrapped message"
            );
            Box::new(ZgfxWrapped { data }) as DvcMessage
        })
        .collect()
}

// --------------- Bridge (DvcProcessor) ---------------

/// Wraps `GraphicsPipelineServer` as a DVC processor, with automatic
/// surface creation on readiness.
///
/// When the client's `CapabilitiesAdvertise` is processed and the server
/// transitions to ready, this bridge auto-creates a surface at the current
/// display dimensions and maps it to output origin (0, 0).
pub struct EgfxBridge {
    shared: SharedEgfx,
}

impl_as_any!(EgfxBridge);

impl DvcProcessor for EgfxBridge {
    #[allow(clippy::unnecessary_literal_bound)]
    fn channel_name(&self) -> &str {
        "Microsoft::Windows::RDS::Graphics"
    }

    fn start(&mut self, channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        tracing::info!(channel_id, "EGFX: DVC channel opened");
        let mut inner = lock_shared(&self.shared);
        inner.dvc_channel_id = Some(channel_id);
        inner.server.start(channel_id).map(zgfx_wrap_messages)
    }

    fn close(&mut self, channel_id: u32) {
        tracing::info!("EGFX: DVC channel closed");
        let mut inner = lock_shared(&self.shared);
        inner.server.close(channel_id);
        inner.ready = false;
        inner.surface_id = None;
        inner.dvc_channel_id = None;
        inner.supports_avc420 = false;
    }

    fn process(&mut self, channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        tracing::trace!(
            channel_id,
            payload_len = payload.len(),
            first_bytes = ?&payload[..payload.len().min(32)],
            "EGFX: incoming DVC payload"
        );
        let mut inner = lock_shared(&self.shared);

        let was_ready = inner.ready;
        let mut messages = inner.server.process(channel_id, payload)?;
        tracing::trace!(
            message_count = messages.len(),
            "EGFX: server.process() returned messages"
        );

        // Sync our ready flag from the server's internal state.
        inner.ready = inner.server.is_ready();
        inner.supports_avc420 = inner.ready; // V8_1 with AVC420 was negotiated if ready

        // On readiness transition: auto-create surface and map to output.
        if !was_ready && inner.ready && inner.surface_id.is_none() {
            let width = inner.width;
            let height = inner.height;

            if let Some(surface_id) = inner.server.create_surface(width, height) {
                inner.server.map_surface_to_output(surface_id, 0, 0);
                inner.surface_id = Some(surface_id);
                tracing::info!(surface_id, width, height, "EGFX: auto-created surface");

                // Append the CreateSurface + MapSurface PDUs to this response.
                messages.extend(inner.server.drain_output());
            } else {
                tracing::error!(width, height, "EGFX: failed to create surface, H.264 delivery disabled");
            }
        }

        Ok(zgfx_wrap_messages(messages))
    }
}

impl DvcServerProcessor for EgfxBridge {}

// --------------- Controller (public handle) ---------------

/// Public handle for the display handler to query EGFX state and send
/// H.264 frames through the DVC channel.
///
/// The server event sender is stored in the shared `EgfxInner` state,
/// so it can be set via [`EgfxEventSetter`] after `RdpServer` construction
/// while the controller has already been moved into the display handler.
///
/// Cloning is cheap (wraps `Arc<Mutex<..>>`), allowing the controller
/// to be shared between `LiveDisplay` (for `request_layout`) and
/// `LiveDisplayUpdates` (for frame delivery).
#[derive(Clone)]
pub struct EgfxController {
    shared: SharedEgfx,
}

impl EgfxController {
    /// Reset EGFX state for a new RDP connection.
    ///
    /// ironrdp-server does not always call `DvcProcessor::close()` when a
    /// client disconnects, leaving stale `ready` / `supports_avc420` flags.
    /// Call this when acquiring display channels for a new connection so the
    /// EGFX handshake starts fresh.
    pub fn reset(&self) {
        let mut inner = lock_shared(&self.shared);
        // Create a fresh pipeline server to avoid stale surfaces/frame IDs.
        inner.server = GraphicsPipelineServer::new(Box::new(ReadyDetectHandler));
        inner.ready = false;
        inner.surface_id = None;
        inner.dvc_channel_id = None;
        inner.supports_avc420 = false;
        inner.needs_keyframe = false;
        tracing::debug!("EGFX: state reset for new connection");
    }

    /// Take and clear the `needs_keyframe` flag.
    ///
    /// Returns `true` if a keyframe should be forced (e.g. after resize),
    /// and resets the flag to `false`.
    #[must_use]
    pub fn take_needs_keyframe(&self) -> bool {
        let mut inner = lock_shared(&self.shared);
        std::mem::take(&mut inner.needs_keyframe)
    }

    /// Whether the EGFX channel is ready to accept H.264 frames.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        lock_shared(&self.shared).ready
    }

    /// Whether the negotiated capabilities include AVC420 (H.264).
    #[must_use]
    pub fn supports_avc420(&self) -> bool {
        lock_shared(&self.shared).supports_avc420
    }

    /// Send an H.264 frame through the EGFX channel.
    ///
    /// Locks the shared state, calls `send_avc420_frame` on the
    /// `GraphicsPipelineServer`, drains the output PDUs, and sends
    /// them via the `ServerEvent::DvcOutput` channel.
    ///
    /// Returns `true` if the frame was queued successfully, `false` if
    /// the channel is not ready, backpressure is active, or the event
    /// sender is not configured.
    #[allow(clippy::cast_possible_truncation)]
    pub fn send_frame(
        &self,
        h264_data: &[u8],
        width: u16,
        height: u16,
        timestamp_ms: u32,
    ) -> bool {
        let mut inner = lock_shared(&self.shared);

        let Some(ref event_tx) = inner.event_tx else {
            tracing::warn!("EGFX: cannot send frame, event sender not configured");
            return false;
        };
        // Clone sender before mutating inner state (borrow checker).
        let event_tx = event_tx.clone();

        let Some(surface_id) = inner.surface_id else {
            return false;
        };

        if inner.server.should_backpressure() {
            tracing::trace!("EGFX: backpressure active, dropping frame");
            return false;
        }

        let region = Avc420Region::full_frame(width, height, EGFX_QP);
        let regions = [region];

        // Pass Annex B H.264 data directly — FreeRDP's OpenH264 decoder
        // accepts start-code prefixed NALUs without conversion.
        let Some(frame_id) =
            inner
                .server
                .send_avc420_frame(surface_id, h264_data, &regions, timestamp_ms)
        else {
            return false;
        };

        let messages = zgfx_wrap_messages(inner.server.drain_output());
        let Some(dvc_channel_id) = inner.dvc_channel_id else {
            return false;
        };

        drop(inner); // Release lock before sending.

        tracing::trace!(frame_id, dvc_channel_id, "EGFX: sending H.264 frame");

        if event_tx
            .send(ServerEvent::DvcOutput {
                dvc_channel_id,
                messages,
            })
            .is_err()
        {
            tracing::warn!("EGFX: event channel closed, cannot send frame");
            return false;
        }

        true
    }

    /// Resize the EGFX surface.
    ///
    /// Deletes the old surface, sends `ResetGraphics`, creates a new
    /// surface at the new dimensions, and maps it to output.
    pub fn resize(&self, width: u16, height: u16) {
        let mut inner = lock_shared(&self.shared);

        let Some(ref event_tx) = inner.event_tx else {
            inner.width = width;
            inner.height = height;
            return;
        };
        let event_tx = event_tx.clone();

        if !inner.ready {
            inner.width = width;
            inner.height = height;
            return;
        }

        inner.width = width;
        inner.height = height;
        inner.needs_keyframe = true;

        inner.server.resize(width, height);

        if let Some(surface_id) = inner.server.create_surface(width, height) {
            inner.server.map_surface_to_output(surface_id, 0, 0);
            inner.surface_id = Some(surface_id);
            tracing::info!(surface_id, width, height, "EGFX: resized surface");
        } else {
            tracing::error!(width, height, "EGFX: failed to create surface after resize");
        }

        let messages = zgfx_wrap_messages(inner.server.drain_output());
        let Some(dvc_channel_id) = inner.dvc_channel_id else {
            return;
        };

        drop(inner);

        let _ = event_tx.send(ServerEvent::DvcOutput {
            dvc_channel_id,
            messages,
        });
    }
}

// --------------- Event Setter ---------------

/// Lightweight handle to set the server event sender in shared EGFX state.
///
/// This is separated from [`EgfxController`] because the controller is
/// moved into the display handler *before* the `RdpServer` is constructed,
/// but the event sender is only available *after* construction.
pub struct EgfxEventSetter {
    shared: SharedEgfx,
}

impl EgfxEventSetter {
    /// Store the server event sender for proactive frame delivery.
    pub fn set_event_sender(&self, sender: mpsc::UnboundedSender<ServerEvent>) {
        let mut inner = lock_shared(&self.shared);
        inner.event_tx = Some(sender);
        tracing::info!("EGFX: event sender configured");
    }
}

// --------------- Bridge Factory ---------------

/// Factory that creates fresh [`EgfxBridge`] instances per RDP connection.
///
/// All bridges share the same `SharedEgfx` state (which is reset by
/// [`EgfxController::reset()`] between connections), allowing the
/// [`EgfxController`] and [`EgfxEventSetter`] to remain valid.
pub struct EgfxBridgeFactory {
    shared: SharedEgfx,
}

impl DvcProcessorFactory for EgfxBridgeFactory {
    fn build(&self) -> Box<dyn DvcProcessor> {
        tracing::debug!("EGFX: creating fresh bridge for new connection");
        Box::new(EgfxBridge {
            shared: Arc::clone(&self.shared),
        })
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn channel_name(&self) -> &str {
        "Microsoft::Windows::RDS::Graphics"
    }
}

// --------------- Factory ---------------

/// Create the EGFX components.
///
/// Returns:
/// - An `EgfxBridgeFactory` to register with `RdpServer::add_dvc_factory`
/// - An `EgfxController` for the display handler to send frames
/// - An `EgfxEventSetter` to inject the server event sender after construction
///
/// The factory creates a fresh [`EgfxBridge`] for each RDP connection,
/// solving the issue where `drain(..)` in `attach_channels` consumed the
/// bridge after the first connection.
pub fn create_egfx(
    width: u16,
    height: u16,
) -> (EgfxBridgeFactory, EgfxController, EgfxEventSetter) {
    let server = GraphicsPipelineServer::new(Box::new(ReadyDetectHandler));

    let shared: SharedEgfx = Arc::new(Mutex::new(EgfxInner {
        server,
        ready: false,
        surface_id: None,
        dvc_channel_id: None,
        supports_avc420: false,
        width,
        height,
        event_tx: None,
        needs_keyframe: false,
    }));

    let factory = EgfxBridgeFactory {
        shared: Arc::clone(&shared),
    };

    let controller = EgfxController {
        shared: Arc::clone(&shared),
    };

    let event_setter = EgfxEventSetter { shared };

    (factory, controller, event_setter)
}

/// Minimal handler that does nothing — readiness is detected by the
/// bridge checking `server.is_ready()` after each `process()` call.
struct ReadyDetectHandler;

impl GraphicsPipelineHandler for ReadyDetectHandler {
    fn capabilities_advertise(&mut self, _pdu: &CapabilitiesAdvertisePdu) {
        tracing::debug!("EGFX: client advertised capabilities");
    }

    fn on_ready(&mut self, negotiated: &CapabilitySet) {
        tracing::info!(?negotiated, "EGFX: channel ready (handler callback)");
    }

    fn on_frame_ack(&mut self, frame_id: u32, queue_depth: u32) {
        tracing::trace!(frame_id, queue_depth, "EGFX: frame acknowledged");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_egfx_returns_factory_and_controller() {
        let (factory, controller, _setter) = create_egfx(1920, 1080);
        let bridge = factory.build();
        assert_eq!(bridge.channel_name(), "Microsoft::Windows::RDS::Graphics");
        assert!(!controller.is_ready());
        assert!(!controller.supports_avc420());

        // Factory can create multiple bridges (one per connection).
        let bridge2 = factory.build();
        assert_eq!(bridge2.channel_name(), "Microsoft::Windows::RDS::Graphics");
    }
}
