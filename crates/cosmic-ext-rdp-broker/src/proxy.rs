use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// Proxy an RDP connection between a client and a per-user server.
///
/// First writes the buffered X.224 Connection Request (the initial
/// packet already read by the broker for routing), then performs
/// bidirectional byte-level proxying until either side closes.
///
/// # Errors
///
/// Returns an error if the initial write or the bidirectional copy fails.
pub async fn proxy_connection(
    mut client: TcpStream,
    server_addr: &str,
    initial_packet: &[u8],
) -> Result<()> {
    let mut server = TcpStream::connect(server_addr)
        .await
        .with_context(|| format!("failed to connect to per-user server at {server_addr}"))?;

    // Forward the buffered X.224 Connection Request that the broker
    // already consumed for cookie extraction.
    server
        .write_all(initial_packet)
        .await
        .context("failed to forward X.224 CR to per-user server")?;

    // Bidirectional proxy: all subsequent bytes are forwarded as-is.
    let (bytes_client_to_server, bytes_server_to_client) =
        tokio::io::copy_bidirectional(&mut client, &mut server)
            .await
            .context("proxy copy error")?;

    tracing::debug!(
        c2s = bytes_client_to_server,
        s2c = bytes_server_to_client,
        "proxy connection closed"
    );

    Ok(())
}
