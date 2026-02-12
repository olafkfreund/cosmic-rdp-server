use anyhow::{bail, Context, Result};

/// Parsed X.224 Connection Request with the raw packet preserved.
#[derive(Debug)]
pub struct X224ConnectionRequest {
    /// The username extracted from the `Cookie: mstshash=<user>` field.
    pub username: Option<String>,
    /// The entire raw packet (TPKT + X.224 CR + cookie + RDP Neg Req).
    /// Must be forwarded to the per-user server verbatim.
    pub raw_packet: Vec<u8>,
}

/// Read and parse an X.224 Connection Request from a TCP stream.
///
/// The X.224 CR is the very first packet an RDP client sends (MS-RDPBCGR
/// 2.2.1.1). It contains a TPKT header (4 bytes) followed by X.224 CR
/// data that may include a routing cookie of the form:
///
/// ```text
/// Cookie: mstshash=<username>\r\n
/// ```
///
/// We buffer the entire packet so it can be forwarded to the per-user
/// server after routing.
///
/// # Errors
///
/// Returns an error if the packet is malformed or the stream closes
/// before a complete packet is received.
pub async fn read_connection_request(
    stream: &mut (impl tokio::io::AsyncReadExt + Unpin),
) -> Result<X224ConnectionRequest> {
    // Read TPKT header (4 bytes): version(1) + reserved(1) + length(2 BE)
    let mut tpkt = [0u8; 4];
    stream
        .read_exact(&mut tpkt)
        .await
        .context("failed to read TPKT header")?;

    if tpkt[0] != 3 {
        bail!("invalid TPKT version: {} (expected 3)", tpkt[0]);
    }

    let total_length = u16::from_be_bytes([tpkt[2], tpkt[3]]) as usize;
    if total_length < 7 {
        bail!("TPKT length too small: {total_length}");
    }
    // Sanity limit: X.224 CR should never exceed a few hundred bytes.
    if total_length > 8192 {
        bail!("TPKT length too large: {total_length}");
    }

    // Read the rest of the packet.
    let payload_len = total_length - 4;
    let mut payload = vec![0u8; payload_len];
    stream
        .read_exact(&mut payload)
        .await
        .context("failed to read X.224 CR payload")?;

    // Verify X.224 CR TPDU code: high nibble of byte 1 should be 0xE (CR).
    if payload.len() < 2 {
        bail!("X.224 payload too short");
    }
    let tpdu_code = payload[1] >> 4;
    if tpdu_code != 0xE {
        bail!("not an X.224 Connection Request (code=0x{tpdu_code:X}, expected 0xE)");
    }

    // Build the complete raw packet.
    let mut raw_packet = Vec::with_capacity(total_length);
    raw_packet.extend_from_slice(&tpkt);
    raw_packet.extend_from_slice(&payload);

    // Extract the routing cookie username.
    let username = extract_cookie_username(&payload);

    Ok(X224ConnectionRequest {
        username,
        raw_packet,
    })
}

/// Extract the username from a `Cookie: mstshash=<user>\r\n` field
/// in the X.224 CR payload.
///
/// The cookie starts after the 6-byte X.224 CR header (LI + CR + DST-REF
/// + SRC-REF + CLASS). We search for the `Cookie:` prefix and parse
///   the `mstshash=` value.
fn extract_cookie_username(payload: &[u8]) -> Option<String> {
    // X.224 CR fixed header is at least 6 bytes.
    if payload.len() <= 6 {
        return None;
    }

    let cookie_data = &payload[6..];
    let cookie_str = std::str::from_utf8(cookie_data).ok()?;

    // Look for "Cookie: mstshash=" (case-insensitive on the Cookie part).
    let lower = cookie_str.to_lowercase();
    let prefix = "cookie: mstshash=";
    let start = lower.find(prefix)?;
    let value_start = start + prefix.len();

    // The cookie value ends at \r\n.
    let remaining = &cookie_str[value_start..];
    let end = remaining.find("\r\n").unwrap_or(remaining.len());
    let username = remaining[..end].trim();

    if username.is_empty() {
        return None;
    }

    // Validate: username should only contain safe characters.
    let is_safe = username
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.');
    if !is_safe {
        tracing::warn!(username, "X.224 cookie contains unsafe characters, ignoring");
        return None;
    }

    // Limit length to prevent abuse.
    if username.len() > 64 {
        tracing::warn!(
            len = username.len(),
            "X.224 cookie username too long, ignoring"
        );
        return None;
    }

    Some(username.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cookie_username() {
        // Simulated X.224 CR payload (after TPKT header).
        // 6 bytes X.224 header + cookie.
        let mut payload = vec![0u8; 6]; // LI, CR|CDT, DST-REF(2), SRC-REF(2)
        payload[0] = 30; // LI (arbitrary, just needs to be > 6 for cookie)
        payload[1] = 0xE0; // CR TPDU code
        payload.extend_from_slice(b"Cookie: mstshash=testuser\r\n");

        let username = extract_cookie_username(&payload);
        assert_eq!(username, Some("testuser".to_string()));
    }

    #[test]
    fn no_cookie_returns_none() {
        let payload = vec![0u8; 6];
        let username = extract_cookie_username(&payload);
        assert_eq!(username, None);
    }

    #[test]
    fn unsafe_username_rejected() {
        let mut payload = vec![0u8; 6];
        payload[1] = 0xE0;
        payload.extend_from_slice(b"Cookie: mstshash=user;rm -rf\r\n");

        let username = extract_cookie_username(&payload);
        assert_eq!(username, None);
    }
}
