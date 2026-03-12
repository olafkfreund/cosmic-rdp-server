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
//! - The upstream [`GfxDvcBridge`] – wraps `GraphicsPipelineServer` as a
//!   `DvcProcessor` inside the DRDYNVC static virtual channel. Handles
//!   client↔server EGFX messages (capability negotiation, frame acks).
//!
//! - [`EgfxController`] – public handle used by `LiveDisplayUpdates` to
//!   check readiness, send H.264 frames, and resize the surface.
//!
//! - [`CosmicGfxFactory`] – implements [`GfxServerFactory`] to create
//!   the bridge/handler and receive the server event sender.

use std::sync::{Arc, Mutex};

use ironrdp_core::encode_vec;
use ironrdp_dvc::DvcMessage;
use ironrdp_egfx::pdu::{Avc420Region, CapabilitiesAdvertisePdu, CapabilitySet};
use ironrdp_egfx::server::{GraphicsPipelineHandler, GraphicsPipelineServer};
use ironrdp_server::{
    EgfxServerMessage, GfxDvcBridge, GfxServerFactory, GfxServerHandle, ServerEvent,
    ServerEventSender,
};
use ironrdp_svc::SvcMessage;
use tokio::sync::mpsc;

/// H.264 quantization parameter for EGFX AVC420 regions.
/// Lower = better quality (18-23 is typical for RDP).
const EGFX_QP: u8 = 22;

/// Convert `Vec<DvcMessage>` (from `drain_output()`) to `Vec<SvcMessage>`
/// for use with `EgfxServerMessage::SendMessages`.
///
/// `DvcMessage = Box<dyn DvcEncode>` and `SvcMessage` wraps `Box<dyn SvcEncode>`.
/// Both trait bounds are `Encode + Send`, but they are separate traits, so we
/// encode to bytes and wrap as `Vec<u8>` which implements `SvcEncode`.
fn dvc_to_svc_messages(dvc_messages: Vec<DvcMessage>) -> Vec<SvcMessage> {
    dvc_messages
        .into_iter()
        .filter_map(|msg: DvcMessage| {
            match encode_vec(msg.as_ref()) {
                Ok(bytes) => Some(SvcMessage::from(bytes)),
                Err(e) => {
                    tracing::error!(?e, "EGFX: failed to encode DVC message to SVC");
                    None
                }
            }
        })
        .collect()
}

/// Shared inner state between the GFX handler, controller, and factory.
struct EgfxInner {
    /// Shared handle to the `GraphicsPipelineServer` (same one inside `GfxDvcBridge`).
    server_handle: Option<GfxServerHandle>,
    ready: bool,
    surface_id: Option<u16>,
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

// --------------- GFX Handler (capability callbacks) ---------------

/// Handler that detects EGFX readiness and auto-creates surfaces.
///
/// When the client's `CapabilitiesAdvertise` is processed and the server
/// transitions to ready, this handler creates a surface at the current
/// display dimensions and maps it to output origin (0, 0).
struct CosmicGfxHandler {
    shared: SharedEgfx,
}

impl GraphicsPipelineHandler for CosmicGfxHandler {
    fn capabilities_advertise(&mut self, _pdu: &CapabilitiesAdvertisePdu) {
        tracing::debug!("EGFX: client advertised capabilities");
    }

    fn on_ready(&mut self, negotiated: &CapabilitySet) {
        tracing::info!(?negotiated, "EGFX: channel ready");
        let mut inner = lock_shared(&self.shared);
        inner.ready = true;
        inner.supports_avc420 = true; // V8_1 with AVC420 was negotiated if ready

        // Auto-create surface on readiness if we have the server handle.
        if inner.surface_id.is_none()
            && let Some(handle) = inner.server_handle.clone()
        {
            let width = inner.width;
            let height = inner.height;
            let mut server = handle.lock().expect("GfxServerHandle mutex poisoned");

            if let Some(surface_id) = server.create_surface(width, height) {
                server.map_surface_to_output(surface_id, 0, 0);
                inner.surface_id = Some(surface_id);
                tracing::info!(surface_id, width, height, "EGFX: auto-created surface");

                // Drain the CreateSurface + MapSurface PDUs and send them.
                let messages = dvc_to_svc_messages(server.drain_output());
                drop(server);
                Self::send_messages(&inner, messages);
            } else {
                tracing::error!(
                    width, height,
                    "EGFX: failed to create surface, H.264 delivery disabled"
                );
            }
        }
    }

    fn on_frame_ack(&mut self, frame_id: u32, queue_depth: u32) {
        tracing::trace!(frame_id, queue_depth, "EGFX: frame acknowledged");
    }
}

impl CosmicGfxHandler {
    /// Send drained messages via the server event channel.
    fn send_messages(inner: &EgfxInner, messages: Vec<SvcMessage>) {
        if messages.is_empty() {
            return;
        }
        if let Some(ref event_tx) = inner.event_tx {
            let _ = event_tx.send(ServerEvent::Egfx(EgfxServerMessage::SendMessages {
                messages,
            }));
        }
    }
}

// --------------- Controller (public handle) ---------------

/// Public handle for the display handler to query EGFX state and send
/// H.264 frames through the DVC channel.
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
    /// Upstream's `GfxDvcBridge` handles DVC lifecycle, but we need to
    /// reset our own tracking state (ready flag, surface ID, etc.)
    /// when a new client connects.
    pub fn reset(&self) {
        let mut inner = lock_shared(&self.shared);
        inner.ready = false;
        inner.surface_id = None;
        inner.supports_avc420 = false;
        inner.needs_keyframe = false;
        // The GraphicsPipelineServer is recreated by the factory for each
        // connection (via build_server_with_handle), so we just clear our handle.
        inner.server_handle = None;
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
    /// them via `ServerEvent::Egfx`.
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
        let inner = lock_shared(&self.shared);

        let Some(ref event_tx) = inner.event_tx else {
            tracing::warn!("EGFX: cannot send frame, event sender not configured");
            return false;
        };
        let event_tx = event_tx.clone();

        let Some(surface_id) = inner.surface_id else {
            return false;
        };

        let Some(ref server_handle) = inner.server_handle else {
            return false;
        };

        let mut server = server_handle.lock().expect("GfxServerHandle mutex poisoned");

        if server.should_backpressure() {
            tracing::trace!("EGFX: backpressure active, dropping frame");
            return false;
        }

        let region = Avc420Region::full_frame(width, height, EGFX_QP);
        let regions = [region];

        // Pass raw Annex B H.264 data directly. FreeRDP's OpenH264 decoder
        // expects Annex B (start-code prefixed: 0x00000001), NOT AVC
        // (length-prefixed). Converting via annex_b_to_avc() causes
        // DecodeFrame2 to fail with state 0x0004.
        let Some(frame_id) =
            server.send_avc420_frame(surface_id, h264_data, &regions, timestamp_ms)
        else {
            return false;
        };

        let messages = dvc_to_svc_messages(server.drain_output());

        drop(server);
        drop(inner);

        tracing::trace!(frame_id, "EGFX: sending H.264 frame");

        if event_tx
            .send(ServerEvent::Egfx(EgfxServerMessage::SendMessages {
                messages,
            }))
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

        let Some(server_handle) = inner.server_handle.clone() else {
            return;
        };

        let mut server = server_handle.lock().expect("GfxServerHandle mutex poisoned");

        server.resize(width, height);

        if let Some(surface_id) = server.create_surface(width, height) {
            server.map_surface_to_output(surface_id, 0, 0);
            inner.surface_id = Some(surface_id);
            tracing::info!(surface_id, width, height, "EGFX: resized surface");
        } else {
            tracing::error!(width, height, "EGFX: failed to create surface after resize");
        }

        let messages = dvc_to_svc_messages(server.drain_output());

        drop(server);
        drop(inner);

        let _ = event_tx.send(ServerEvent::Egfx(EgfxServerMessage::SendMessages {
            messages,
        }));
    }
}

// --------------- GFX Factory ---------------

/// Factory that creates EGFX graphics pipeline components per connection.
///
/// Implements [`GfxServerFactory`] (required by upstream's builder) and
/// [`ServerEventSender`] (to receive the event channel after construction).
pub struct CosmicGfxFactory {
    shared: SharedEgfx,
}

impl GfxServerFactory for CosmicGfxFactory {
    fn build_gfx_handler(&self) -> Box<dyn GraphicsPipelineHandler> {
        Box::new(CosmicGfxHandler {
            shared: Arc::clone(&self.shared),
        })
    }

    fn build_server_with_handle(&self) -> Option<(GfxDvcBridge, GfxServerHandle)> {
        let handler = self.build_gfx_handler();
        let server = GraphicsPipelineServer::new(handler);
        let handle: GfxServerHandle = Arc::new(Mutex::new(server));
        let bridge = GfxDvcBridge::new(Arc::clone(&handle));

        // Store the handle so the controller can access it.
        let mut inner = lock_shared(&self.shared);
        inner.server_handle = Some(Arc::clone(&handle));

        Some((bridge, handle))
    }
}

impl ServerEventSender for CosmicGfxFactory {
    fn set_sender(&mut self, sender: mpsc::UnboundedSender<ServerEvent>) {
        let mut inner = lock_shared(&self.shared);
        inner.event_tx = Some(sender);
        tracing::info!("EGFX: event sender configured");
    }
}

// --------------- Public Factory Function ---------------

/// Create the EGFX components.
///
/// Returns:
/// - A [`CosmicGfxFactory`] to pass to `RdpServer::builder().with_gfx_factory()`
/// - An [`EgfxController`] for the display handler to send frames
///
/// The factory creates a fresh `GfxDvcBridge` + `GraphicsPipelineServer` for
/// each RDP connection via [`GfxServerFactory::build_server_with_handle()`].
pub fn create_egfx(width: u16, height: u16) -> (CosmicGfxFactory, EgfxController) {
    let shared: SharedEgfx = Arc::new(Mutex::new(EgfxInner {
        server_handle: None,
        ready: false,
        surface_id: None,
        supports_avc420: false,
        width,
        height,
        event_tx: None,
        needs_keyframe: false,
    }));

    let factory = CosmicGfxFactory {
        shared: Arc::clone(&shared),
    };

    let controller = EgfxController { shared };

    (factory, controller)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_egfx_returns_factory_and_controller() {
        let (_factory, controller) = create_egfx(1920, 1080);
        assert!(!controller.is_ready());
        assert!(!controller.supports_avc420());
    }
}
