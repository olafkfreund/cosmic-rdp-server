//! Clipboard backend for sharing clipboard content between the local
//! Wayland session and the remote RDP client.
//!
//! Uses [`arboard`] for system clipboard access and implements the
//! [`ironrdp_cliprdr`] backend traits so that `ironrdp-server` can
//! negotiate the CLIPRDR virtual channel automatically.
//!
//! Only plain-text clipboard (`CF_UNICODETEXT` / `CF_TEXT`) is supported.

use ironrdp_cliprdr::backend::{
    CliprdrBackend, CliprdrBackendFactory, ClipboardMessage,
};
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags,
    FileContentsRequest, FileContentsResponse, FormatDataRequest, FormatDataResponse,
    LockDataId, OwnedFormatDataResponse,
};
use ironrdp_server::{CliprdrServerFactory, ServerEvent, ServerEventSender};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Backend (one per RDP connection)
// ---------------------------------------------------------------------------

/// Clipboard backend that reads/writes the local system clipboard via
/// [`arboard::Clipboard`].
#[derive(Debug)]
pub struct LocalClipboardBackend {
    /// Channel to send clipboard events back to the ironrdp server.
    event_tx: mpsc::UnboundedSender<ServerEvent>,
    /// Formats that the remote client currently offers.
    remote_formats: Vec<ClipboardFormat>,
}

impl LocalClipboardBackend {
    fn new(event_tx: mpsc::UnboundedSender<ServerEvent>) -> Self {
        Self {
            event_tx,
            remote_formats: Vec::new(),
        }
    }

    /// Send a clipboard message to the ironrdp server event loop.
    fn send(&self, msg: ClipboardMessage) {
        if self.event_tx.send(ServerEvent::Clipboard(msg)).is_err() {
            tracing::warn!("Clipboard event channel closed");
        }
    }

    /// Build the text format list we advertise to the remote.
    fn text_formats() -> Vec<ClipboardFormat> {
        vec![
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
            ClipboardFormat::new(ClipboardFormatId::CF_TEXT),
        ]
    }
}

impl ironrdp_core::AsAny for LocalClipboardBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl CliprdrBackend for LocalClipboardBackend {
    #[allow(clippy::unnecessary_literal_bound)]
    fn temporary_directory(&self) -> &str {
        "/tmp"
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::empty()
    }

    fn on_ready(&mut self) {
        tracing::info!("CLIPRDR channel ready");
        // Advertise our local clipboard content to the remote.
        self.on_request_format_list();
    }

    fn on_request_format_list(&mut self) {
        // Check if the local clipboard has text content.
        match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
            Ok(text) if !text.is_empty() => {
                tracing::debug!(len = text.len(), "Advertising local clipboard text");
                self.send(ClipboardMessage::SendInitiateCopy(Self::text_formats()));
            }
            _ => {
                // Nothing to offer (or clipboard unavailable).
                tracing::debug!("No text in local clipboard to advertise");
            }
        }
    }

    fn on_process_negotiated_capabilities(
        &mut self,
        capabilities: ClipboardGeneralCapabilityFlags,
    ) {
        tracing::debug!(?capabilities, "Negotiated clipboard capabilities");
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        // Remote client has new clipboard content. Store the format list so we
        // can request data when the user pastes locally.
        tracing::debug!(?available_formats, "Remote clipboard updated");
        self.remote_formats = available_formats.to_vec();

        // If the remote offers text, request it immediately so we can push it
        // to the local clipboard.
        let has_unicode = available_formats
            .iter()
            .any(|f| f.id == ClipboardFormatId::CF_UNICODETEXT);
        let has_text = available_formats
            .iter()
            .any(|f| f.id == ClipboardFormatId::CF_TEXT);

        if has_unicode {
            self.send(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_UNICODETEXT,
            ));
        } else if has_text {
            self.send(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_TEXT,
            ));
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        // Remote wants to paste our local clipboard content.
        tracing::debug!(?request, "Remote requesting local clipboard data");

        let response = match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
            Ok(text) => {
                if request.format == ClipboardFormatId::CF_UNICODETEXT {
                    OwnedFormatDataResponse::new_unicode_string(&text)
                } else if request.format == ClipboardFormatId::CF_TEXT {
                    OwnedFormatDataResponse::new_string(&text)
                } else {
                    tracing::debug!(format = ?request.format, "Unsupported format requested");
                    OwnedFormatDataResponse::new_error()
                }
            }
            Err(e) => {
                tracing::warn!("Failed to read local clipboard: {e}");
                OwnedFormatDataResponse::new_error()
            }
        };

        self.send(ClipboardMessage::SendFormatData(response));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        // Remote sent us clipboard data (text). Write it to the local clipboard.
        if response.is_error() {
            tracing::debug!("Remote sent clipboard error response");
            return;
        }

        // Try UTF-16LE first (CF_UNICODETEXT), fall back to ANSI (CF_TEXT).
        let text = decode_utf16le_text(response.data())
            .or_else(|| decode_ansi_text(response.data()));

        match text {
            Some(s) => {
                match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(s.clone())) {
                    Ok(()) => {
                        tracing::debug!(len = s.len(), "Wrote remote text to local clipboard");
                    }
                    Err(e) => {
                        tracing::warn!("Failed to write to local clipboard: {e}");
                    }
                }
            }
            None => {
                tracing::debug!("Empty or undecodable clipboard data from remote");
            }
        }
    }

    fn on_file_contents_request(&mut self, _request: FileContentsRequest) {
        tracing::debug!("File contents request ignored (not supported)");
    }

    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {
        tracing::debug!("File contents response ignored (not supported)");
    }

    fn on_lock(&mut self, _data_id: LockDataId) {}

    fn on_unlock(&mut self, _data_id: LockDataId) {}
}

// ---------------------------------------------------------------------------
// Factory (shared across connections)
// ---------------------------------------------------------------------------

/// Factory that creates [`LocalClipboardBackend`] instances for each RDP
/// connection and holds the server event sender.
#[derive(Debug)]
pub struct LocalClipboardFactory {
    event_tx: Option<mpsc::UnboundedSender<ServerEvent>>,
}

impl LocalClipboardFactory {
    pub fn new() -> Self {
        Self { event_tx: None }
    }
}

impl CliprdrBackendFactory for LocalClipboardFactory {
    fn build_cliprdr_backend(&self) -> Box<dyn CliprdrBackend> {
        let tx = self
            .event_tx
            .clone()
            .expect("set_sender must be called before build_cliprdr_backend");
        Box::new(LocalClipboardBackend::new(tx))
    }
}

impl ServerEventSender for LocalClipboardFactory {
    fn set_sender(&mut self, sender: mpsc::UnboundedSender<ServerEvent>) {
        self.event_tx = Some(sender);
    }
}

impl CliprdrServerFactory for LocalClipboardFactory {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Decode clipboard data bytes as UTF-16LE text (`CF_UNICODETEXT` format).
///
/// RDP clipboard text is always UTF-16LE with a null terminator.
fn decode_utf16le_text(data: &[u8]) -> Option<String> {
    if data.len() < 2 || data.len() % 2 != 0 {
        return None;
    }

    let u16_iter = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]));

    // Collect until null terminator
    let chars: Vec<u16> = u16_iter.take_while(|&c| c != 0).collect();
    let s = String::from_utf16(&chars).ok()?;
    if s.is_empty() { None } else { Some(s) }
}

/// Decode clipboard data bytes as ANSI/UTF-8 text (`CF_TEXT` format).
fn decode_ansi_text(data: &[u8]) -> Option<String> {
    if data.is_empty() {
        return None;
    }
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    let s = String::from_utf8(data[..end].to_vec()).ok()?;
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_utf16le() {
        // "Hello" in UTF-16LE + null terminator
        let data: Vec<u8> = "Hello"
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .chain([0, 0])
            .collect();
        assert_eq!(decode_utf16le_text(&data), Some("Hello".to_string()));
    }

    #[test]
    fn decode_ansi() {
        let data = b"Hello\0";
        assert_eq!(decode_ansi_text(data), Some("Hello".to_string()));
    }

    #[test]
    fn decode_empty_returns_none() {
        assert_eq!(decode_utf16le_text(&[]), None);
        assert_eq!(decode_ansi_text(&[]), None);
    }
}
