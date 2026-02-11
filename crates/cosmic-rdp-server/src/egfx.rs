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

use ironrdp_core::impl_as_any;
use ironrdp_dvc::{DvcMessage, DvcProcessor, DvcServerProcessor};
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
        inner.server.start(channel_id)
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
        let mut inner = lock_shared(&self.shared);

        let was_ready = inner.ready;
        let mut messages = inner.server.process(channel_id, payload)?;

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

        Ok(messages)
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
pub struct EgfxController {
    shared: SharedEgfx,
}

impl EgfxController {
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

        let Some(frame_id) =
            inner
                .server
                .send_avc420_frame(surface_id, h264_data, &regions, timestamp_ms)
        else {
            return false;
        };

        let messages = inner.server.drain_output();
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

        inner.server.resize(width, height);

        if let Some(surface_id) = inner.server.create_surface(width, height) {
            inner.server.map_surface_to_output(surface_id, 0, 0);
            inner.surface_id = Some(surface_id);
            tracing::info!(surface_id, width, height, "EGFX: resized surface");
        } else {
            tracing::error!(width, height, "EGFX: failed to create surface after resize");
        }

        let messages = inner.server.drain_output();
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

// --------------- Factory ---------------

/// Create the EGFX components.
///
/// Returns:
/// - A boxed `DvcProcessor` to register with `DrdynvcServer`
/// - An `EgfxController` for the display handler to send frames
/// - An `EgfxEventSetter` to inject the server event sender after construction
///
/// The handler callbacks (readiness, frame acks) are minimal — the bridge
/// detects readiness by checking `server.is_ready()` after each `process()`
/// call, avoiding circular `Arc<Mutex<>>` references.
pub fn create_egfx(
    width: u16,
    height: u16,
) -> (Box<dyn DvcProcessor>, EgfxController, EgfxEventSetter) {
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
    }));

    let bridge = EgfxBridge {
        shared: Arc::clone(&shared),
    };

    let controller = EgfxController {
        shared: Arc::clone(&shared),
    };

    let event_setter = EgfxEventSetter { shared };

    (Box::new(bridge), controller, event_setter)
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
    fn create_egfx_returns_bridge_and_controller() {
        let (bridge, controller, _setter) = create_egfx(1920, 1080);
        assert_eq!(bridge.channel_name(), "Microsoft::Windows::RDS::Graphics");
        assert!(!controller.is_ready());
        assert!(!controller.supports_avc420());
    }
}
