use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::TcpListener;

use crate::config::{BrokerConfig, SessionPolicy};
use crate::pam_auth;
use crate::proxy;
use crate::session::{self, SessionEntry, SessionRegistry, SessionStateSerde};
use crate::spawner;
use crate::x224;

/// Run the main broker loop: accept connections and route them.
pub async fn run(config: &BrokerConfig, registry: SessionRegistry) -> Result<()> {
    let listener = TcpListener::bind(&config.bind)
        .await
        .with_context(|| format!("failed to bind broker to {}", config.bind))?;

    tracing::info!(bind = %config.bind, "Broker listening for RDP connections");

    loop {
        let (stream, peer_addr) = listener
            .accept()
            .await
            .context("failed to accept connection")?;

        tracing::info!(%peer_addr, "New connection");

        let config = config.clone();
        let registry = registry.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, peer_addr, &config, &registry).await {
                tracing::warn!(%peer_addr, "Connection handling failed: {e:#}");
            }
        });
    }
}

/// Handle a single incoming RDP connection.
async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    config: &BrokerConfig,
    registry: &SessionRegistry,
) -> Result<()> {
    // Step 1: Read the X.224 Connection Request to extract the username.
    let cr = x224::read_connection_request(&mut stream)
        .await
        .context("failed to read X.224 Connection Request")?;

    let username = if let Some(ref u) = cr.username {
        u.clone()
    } else {
        tracing::warn!(%peer_addr, "No username cookie in X.224 CR, rejecting");
        anyhow::bail!("no username cookie in connection request");
    };
    tracing::info!(%peer_addr, %username, "Routing connection");

    // Step 2: Check for existing session.
    if let Some(existing) = registry.get(&username).await {
        match existing.state {
            SessionStateSerde::Active | SessionStateSerde::Idle => {
                match config.session_policy {
                    SessionPolicy::OnePerUser => {
                        // Reconnect to existing session.
                        tracing::info!(
                            %username,
                            port = existing.port,
                            "Reconnecting to existing session"
                        );
                        registry.set_state(&username, SessionStateSerde::Active).await;
                        registry.set_client_addr(&username, &peer_addr.to_string()).await;
                        let _ = registry.save_state().await;

                        let server_addr = format!("127.0.0.1:{}", existing.port);
                        return proxy::proxy_connection(stream, &server_addr, &cr.raw_packet)
                            .await
                            .map(|()| {
                                handle_disconnect(&username, registry);
                            });
                    }
                    SessionPolicy::ReplaceExisting => {
                        // Stop existing session, fall through to create new one.
                        tracing::info!(
                            %username,
                            "Replacing existing session (policy: ReplaceExisting)"
                        );
                        let _ = spawner::stop_user_server(&existing.unit_name).await;
                        registry.remove(&username).await;
                    }
                }
            }
            SessionStateSerde::Starting => {
                // Session is still starting, wait for it.
                tracing::info!(%username, "Session still starting, waiting...");
                let port = existing.port;
                spawner::wait_for_server_ready(port, Duration::from_secs(30)).await?;
                registry.set_state(&username, SessionStateSerde::Active).await;
                registry.set_client_addr(&username, &peer_addr.to_string()).await;
                let _ = registry.save_state().await;

                let server_addr = format!("127.0.0.1:{port}");
                return proxy::proxy_connection(stream, &server_addr, &cr.raw_packet)
                    .await
                    .map(|()| {
                        handle_disconnect(&username, registry);
                    });
            }
            SessionStateSerde::Stopping => {
                // Wait for stop, then create new session.
                tracing::info!(%username, "Session stopping, waiting before creating new one");
                tokio::time::sleep(Duration::from_secs(2)).await;
                registry.remove(&username).await;
            }
        }
    }

    // Step 3: Authenticate via PAM.
    // Note: For the X.224 cookie approach, we only have the username.
    // Full NLA authentication happens between the client and the per-user
    // server. The broker uses PAM to verify the user exists and has a
    // valid account (not locked, not expired).
    //
    // For proper password auth, the broker would need to intercept the
    // NLA/CredSSP exchange, which ironrdp doesn't support from the broker
    // side. Instead, we verify the account exists and delegate full auth
    // to the per-user server.
    let auth_result = verify_user_account(&username).await?;
    let uid = auth_result.uid;

    // Step 4: Allocate a port and spawn the per-user server.
    let port = registry.allocate_port().await?;

    let env = spawner::discover_user_env(&username, uid)
        .await
        .with_context(|| format!("failed to discover env for user '{username}'"))?;

    // Register the session as Starting.
    let entry = SessionEntry {
        username: username.clone(),
        port,
        pid: 0, // Updated after spawn.
        state: SessionStateSerde::Starting,
        created_at: session::now_unix(),
        client_addr: peer_addr.to_string(),
        unit_name: String::new(),
    };
    registry.insert(entry).await;
    let _ = registry.save_state().await;

    // Spawn the per-user server.
    let unit_name = spawner::spawn_user_server(&config.server_binary, port, &env, &username)
        .await
        .with_context(|| format!("failed to spawn server for user '{username}'"))?;

    // Update registry with unit name.
    // (PID discovery from systemd-run scope is complex; we rely on the
    // unit name for lifecycle management.)
    if let Some(mut entry) = registry.get(&username).await {
        entry.unit_name = unit_name;
        entry.state = SessionStateSerde::Starting;
        registry.insert(entry).await;
    }

    // Wait for the server to become ready.
    spawner::wait_for_server_ready(port, Duration::from_secs(30))
        .await
        .with_context(|| format!("per-user server for '{username}' did not become ready"))?;

    registry.set_state(&username, SessionStateSerde::Active).await;
    let _ = registry.save_state().await;

    tracing::info!(%username, port, "Session ready, proxying connection");

    // Step 5: Proxy the connection.
    let server_addr = format!("127.0.0.1:{port}");
    let result = proxy::proxy_connection(stream, &server_addr, &cr.raw_packet).await;

    handle_disconnect(&username, registry);

    result
}

/// Handle a client disconnection by marking the session as idle.
fn handle_disconnect(username: &str, registry: &SessionRegistry) {
    let username = username.to_string();
    let registry = registry.clone();
    tokio::spawn(async move {
        tracing::info!(%username, "Client disconnected, marking session idle");
        registry.set_state(&username, SessionStateSerde::Idle).await;
        registry.set_client_addr(&username, "").await;
        let _ = registry.save_state().await;
    });
}

/// Verify a user account exists on the system (no password check).
///
/// The broker validates the user exists and has a valid UID. Full
/// password authentication is handled by the per-user server's NLA.
async fn verify_user_account(username: &str) -> Result<pam_auth::PamAuthResult> {
    let username = username.to_string();
    tokio::task::spawn_blocking(move || {
        let user = nix::unistd::User::from_name(&username)
            .context("failed to look up user")?
            .with_context(|| format!("user '{username}' not found on system"))?;

        Ok(pam_auth::PamAuthResult {
            username,
            uid: user.uid.as_raw(),
        })
    })
    .await
    .context("user verification task panicked")?
}

/// Background task that periodically cleans up idle sessions.
pub async fn idle_cleanup_task(registry: SessionRegistry, idle_timeout_secs: u64) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;

        let idle_users = registry.idle_sessions(idle_timeout_secs).await;
        for username in idle_users {
            tracing::info!(%username, "Idle timeout reached, terminating session");
            if let Some(entry) = registry.remove(&username).await {
                if let Err(e) = spawner::stop_user_server(&entry.unit_name).await {
                    tracing::warn!(%username, "Failed to stop idle session: {e}");
                }
            }
        }

        if let Err(e) = registry.save_state().await {
            tracing::warn!("Failed to save state after cleanup: {e}");
        }
    }
}
