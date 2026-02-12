use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// Environment variables needed for a per-user COSMIC session.
#[derive(Debug, Clone)]
pub struct UserSessionEnv {
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub wayland_display: String,
    pub xdg_runtime_dir: String,
    pub dbus_session_bus_address: String,
}

/// Discover the graphical session environment for a user by querying
/// logind via D-Bus (system bus).
///
/// Looks for the user's active graphical session and extracts
/// `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, and
/// `DBUS_SESSION_BUS_ADDRESS`.
pub async fn discover_user_env(username: &str, uid: u32) -> Result<UserSessionEnv> {
    let xdg_runtime_dir = format!("/run/user/{uid}");

    // Try to read environment from logind session.
    // We look for session files in /run/systemd/sessions/ or query
    // loginctl show-session.
    let wayland_display = discover_wayland_display(uid, &xdg_runtime_dir).await?;
    let dbus_addr = format!("unix:path={xdg_runtime_dir}/bus");

    // Get user's home directory and GID.
    let user_info = tokio::task::spawn_blocking({
        let username = username.to_string();
        move || -> Result<(String, u32)> {
            let user = nix::unistd::User::from_name(&username)
                .context("failed to look up user")?
                .with_context(|| format!("user '{username}' not found"))?;
            Ok((
                user.dir.to_string_lossy().to_string(),
                user.gid.as_raw(),
            ))
        }
    })
    .await
    .context("spawn_blocking join error")??;

    Ok(UserSessionEnv {
        uid,
        gid: user_info.1,
        home: user_info.0,
        wayland_display,
        xdg_runtime_dir,
        dbus_session_bus_address: dbus_addr,
    })
}

/// Discover the Wayland display for a user.
///
/// Tries several strategies:
/// 1. Look for `wayland-*` sockets in `XDG_RUNTIME_DIR`
/// 2. Query loginctl for the session's `WAYLAND_DISPLAY` property
async fn discover_wayland_display(uid: u32, xdg_runtime_dir: &str) -> Result<String> {
    // Strategy 1: Look for wayland sockets in the runtime dir.
    let runtime_dir = xdg_runtime_dir.to_string();
    let result = tokio::task::spawn_blocking(move || -> Option<String> {
        let dir = std::fs::read_dir(&runtime_dir).ok()?;
        for entry in dir.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("wayland-") && !name_str.ends_with(".lock") {
                return Some(name_str.to_string());
            }
        }
        None
    })
    .await
    .context("spawn_blocking join error")?;

    if let Some(display) = result {
        return Ok(display);
    }

    // Strategy 2: Query loginctl.
    let output = tokio::process::Command::new("loginctl")
        .args(["show-user", &uid.to_string(), "--property=Display", "--value"])
        .output()
        .await
        .context("failed to run loginctl")?;

    if output.status.success() {
        let session_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !session_id.is_empty() {
            // Query session for WAYLAND_DISPLAY.
            let output = tokio::process::Command::new("loginctl")
                .args([
                    "show-session",
                    &session_id,
                    "--property=Type",
                    "--value",
                ])
                .output()
                .await
                .context("failed to query loginctl session type")?;

            if output.status.success() {
                let session_type = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if session_type == "wayland" {
                    // Default to wayland-0 for Wayland sessions.
                    return Ok("wayland-0".to_string());
                }
            }
        }
    }

    bail!("could not discover Wayland display for UID {uid}")
}

/// Spawn a per-user `cosmic-ext-rdp-server` instance via `systemd-run`.
///
/// Creates a transient systemd user unit that runs the server process
/// as the specified user. The server binds to `127.0.0.1:<port>` with
/// authentication disabled (the broker handles auth).
///
/// Returns the systemd transient unit name.
pub async fn spawn_user_server(
    server_binary: &Path,
    port: u16,
    env: &UserSessionEnv,
    username: &str,
) -> Result<String> {
    let unit_name = format!("cosmic-ext-rdp-session-{username}");

    let output = tokio::process::Command::new("systemd-run")
        .args([
            "--uid",
            &env.uid.to_string(),
            "--gid",
            &env.gid.to_string(),
            "--unit",
            &unit_name,
            "--scope",
            "--slice",
            "cosmic-ext-rdp-sessions.slice",
            "--setenv",
            &format!("WAYLAND_DISPLAY={}", env.wayland_display),
            "--setenv",
            &format!("XDG_RUNTIME_DIR={}", env.xdg_runtime_dir),
            "--setenv",
            &format!("DBUS_SESSION_BUS_ADDRESS={}", env.dbus_session_bus_address),
            "--setenv",
            &format!("HOME={}", env.home),
            "--setenv",
            &format!("USER={username}"),
            "--setenv",
            "RUST_LOG=info",
            "--",
            &server_binary.to_string_lossy(),
            "--addr",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .output()
        .await
        .context("failed to run systemd-run")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("systemd-run failed: {stderr}");
    }

    tracing::info!(
        username,
        port,
        uid = env.uid,
        unit = %unit_name,
        "Spawned per-user server"
    );

    Ok(unit_name)
}

/// Wait for a per-user server to become ready by polling its TCP port.
///
/// Uses exponential backoff starting at 10ms up to 10 seconds.
pub async fn wait_for_server_ready(port: u16, timeout: Duration) -> Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let start = std::time::Instant::now();
    let mut delay = Duration::from_millis(10);
    let max_delay = Duration::from_secs(2);

    loop {
        match tokio::net::TcpStream::connect(&addr).await {
            Ok(_) => {
                tracing::debug!(port, "Per-user server is ready");
                return Ok(());
            }
            Err(_) if start.elapsed() > timeout => {
                bail!("per-user server on port {port} not ready after {timeout:?}");
            }
            Err(_) => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(max_delay);
            }
        }
    }
}

/// Stop a per-user server by stopping its systemd scope unit.
pub async fn stop_user_server(unit_name: &str) -> Result<()> {
    let output = tokio::process::Command::new("systemctl")
        .args(["stop", unit_name])
        .output()
        .await
        .context("failed to run systemctl stop")?;

    if output.status.success() {
        tracing::info!(unit = %unit_name, "Stopped per-user server");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(unit = %unit_name, "systemctl stop failed: {stderr}");
    }

    Ok(())
}
