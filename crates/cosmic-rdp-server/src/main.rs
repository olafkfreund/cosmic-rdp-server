use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use rdp_encode::{EncoderConfig, GstEncoder};

mod clipboard;
mod config;
mod dbus;
mod egfx;
mod server;
mod sound;
mod tls;

/// RDP server for the COSMIC Desktop Environment.
///
/// Allows remote access to COSMIC desktops using standard RDP clients
/// (Windows `mstsc.exe`, `FreeRDP`, Remmina).
#[derive(Parser, Debug)]
#[command(name = "cosmic-rdp-server", version, about)]
struct Cli {
    /// Address to bind the RDP server to.
    #[arg(long)]
    addr: Option<String>,

    /// Port to listen on.
    #[arg(long)]
    port: Option<u16>,

    /// Path to TLS certificate file (PEM format).
    /// If not provided, a self-signed certificate will be generated.
    #[arg(long)]
    cert: Option<PathBuf>,

    /// Path to TLS private key file (PEM format).
    /// Required if --cert is provided.
    #[arg(long)]
    key: Option<PathBuf>,

    /// Path to configuration file (TOML).
    #[arg(long, short)]
    config: Option<PathBuf>,

    /// Use a static blue screen instead of live capture (for testing).
    #[arg(long)]
    static_display: bool,

    /// Swap Red and Blue color channels (use if colors look inverted).
    #[arg(long)]
    swap_colors: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let mut cfg = load_and_merge_config(&cli)?;

    // Start D-Bus server for IPC with the settings UI.
    let dbus_state = rdp_dbus::server::RdpServerState::new(cfg.bind.to_string());
    let (_dbus_conn, mut dbus_cmd_rx) =
        dbus::start_dbus_server(dbus_state.clone()).await?;

    loop {
        // Security check: refuse to bind to non-localhost without authentication.
        // Checked every loop iteration (including after config reload) to prevent
        // auth bypass via D-Bus Reload with a modified config.
        if !cfg.auth.enable && !is_localhost(cfg.bind.ip()) {
            bail!(
                "auth.enable must be true when binding to non-localhost address {}. \
                 Set auth.enable=true with credentials, or bind to 127.0.0.1/::1 for local-only access.",
                cfg.bind.ip()
            );
        }

        let tls_ctx = setup_tls(&cfg)?;
        let auth = setup_auth(&cfg)?;

        let make_cliprdr = || -> Option<Box<dyn ironrdp_server::CliprdrServerFactory>> {
            if cfg.clipboard.enable {
                tracing::info!("Clipboard sharing enabled");
                Some(Box::new(clipboard::LocalClipboardFactory::new()))
            } else {
                None
            }
        };

        let make_sound = || -> Option<Box<dyn ironrdp_server::SoundServerFactory>> {
            if cfg.audio.enable {
                tracing::info!(
                    channels = cfg.audio.channels,
                    sample_rate = cfg.audio.sample_rate,
                    "Audio forwarding enabled (RDPSND)"
                );
                Some(Box::new(sound::PipeWireAudioFactory::new(
                    cfg.audio.channels,
                    cfg.audio.sample_rate,
                )))
            } else {
                None
            }
        };

        tracing::info!(bind = %cfg.bind, "Starting cosmic-rdp-server");
        dbus_state.set_status(rdp_dbus::types::ServerStatus::Running).await;

        let result = if cfg.static_display {
            tracing::info!("Using static display with EGFX color test pattern");
            let (egfx_factory, egfx_controller, egfx_event_setter) =
                egfx::create_egfx(1920, 1080);
            let rdp_server = server::build_server(
                cfg.bind, &tls_ctx, auth.as_ref(), make_cliprdr(), make_sound(),
                Some(Box::new(egfx_factory)),
            );
            egfx_event_setter.set_event_sender(rdp_server.event_sender().clone());
            // Spawn background H.264 encoding task that sends a color test
            // pattern (RGBW quadrants) via EGFX when the client negotiates
            // AVC420. This allows testing the full encode→decode color
            // pipeline without needing live screen capture.
            tokio::spawn(static_egfx_task(egfx_controller, 1920, 1080));
            run_with_shutdown(rdp_server, &mut dbus_cmd_rx).await
        } else {
            run_live_or_fallback(
                &cfg, &tls_ctx, auth.as_ref(), &make_cliprdr, &make_sound, &mut dbus_cmd_rx,
            )
            .await
        };

        match result {
            Ok(ShutdownReason::Reload) => {
                tracing::info!("Reloading configuration");
                cfg = load_and_merge_config(&cli)?;
            }
            Ok(ShutdownReason::Stop | ShutdownReason::Signal) => {
                dbus_state.set_status(rdp_dbus::types::ServerStatus::Stopped).await;
                tracing::info!("Server stopped");
                return Ok(());
            }
            Err(e) => {
                dbus_state.set_status(rdp_dbus::types::ServerStatus::Error).await;
                return Err(e);
            }
        }
    }
}

/// Load config from file and apply CLI overrides.
fn load_and_merge_config(cli: &Cli) -> Result<config::ServerConfig> {
    let mut cfg = config::load_config(cli.config.as_deref())?;

    if let Some(addr) = &cli.addr {
        let port = cli.port.unwrap_or(cfg.bind.port());
        cfg.bind = format!("{addr}:{port}")
            .parse()
            .context("invalid bind address")?;
    } else if let Some(port) = cli.port {
        cfg.bind.set_port(port);
    }

    if let Some(cert) = &cli.cert {
        cfg.cert_path = Some(cert.clone());
    }
    if let Some(key) = &cli.key {
        cfg.key_path = Some(key.clone());
    }
    if cli.static_display {
        cfg.static_display = true;
    }
    if cli.swap_colors {
        cfg.capture.swap_colors = true;
    }

    match (&cfg.cert_path, &cfg.key_path) {
        (Some(_), None) => bail!("--cert requires --key (or set key_path in config)"),
        (None, Some(_)) => bail!("--key requires --cert (or set cert_path in config)"),
        _ => {}
    }

    Ok(cfg)
}

/// Initialise TLS from files or generate self-signed.
fn setup_tls(cfg: &config::ServerConfig) -> Result<tls::TlsContext> {
    match (&cfg.cert_path, &cfg.key_path) {
        (Some(cert), Some(key)) => tls::load_from_files(cert, key),
        _ => tls::generate_self_signed(cfg.bind.ip()),
    }
}

/// Build auth credentials if NLA is enabled.
fn setup_auth(cfg: &config::ServerConfig) -> Result<Option<server::AuthCredentials>> {
    if !cfg.auth.enable {
        return Ok(None);
    }
    if cfg.auth.username.is_empty() {
        bail!("auth.enable is true but auth.username is empty");
    }
    if cfg.auth.password.is_empty() {
        bail!("auth.enable is true but auth.password is empty");
    }
    tracing::info!(username = %cfg.auth.username, "NLA authentication enabled");
    Ok(Some(server::AuthCredentials {
        username: cfg.auth.username.clone(),
        password: cfg.auth.password.clone(),
        domain: cfg.auth.domain.clone(),
    }))
}

/// Reason the server shut down.
enum ShutdownReason {
    /// Unix signal received.
    Signal,
    /// D-Bus `Reload` command.
    Reload,
    /// D-Bus `Stop` command.
    Stop,
}

/// Try live capture, fall back to static blue screen on failure.
async fn run_live_or_fallback(
    cfg: &config::ServerConfig,
    tls_ctx: &tls::TlsContext,
    auth: Option<&server::AuthCredentials>,
    make_cliprdr: &dyn Fn() -> Option<Box<dyn ironrdp_server::CliprdrServerFactory>>,
    make_sound: &dyn Fn() -> Option<Box<dyn ironrdp_server::SoundServerFactory>>,
    dbus_cmd_rx: &mut tokio::sync::mpsc::Receiver<rdp_dbus::server::DaemonCommand>,
) -> Result<ShutdownReason> {
    let restore_token = load_restore_token();
    match rdp_capture::start_capture(
        restore_token.as_deref(),
        cfg.capture.channel_capacity,
        cfg.capture.swap_colors,
    )
    .await
    {
        Ok((capture_handle, event_rx, desktop_info)) => {
            // Persist the restore token so subsequent service restarts can
            // skip the ScreenCast portal dialog.
            if let Some(ref token) = desktop_info.restore_token {
                save_restore_token(token);
            }

            tracing::info!(
                width = desktop_info.width,
                height = desktop_info.height,
                node_id = desktop_info.node_id,
                "Live screen capture active"
            );

            let mut live_display = server::LiveDisplay::new(event_rx, &desktop_info);

            // Create EGFX components for H.264 delivery via DVC.
            let (egfx_factory, egfx_controller, egfx_event_setter) =
                egfx::create_egfx(desktop_info.width, desktop_info.height);
            live_display.set_egfx(egfx_controller);

            let input_handler = match rdp_input::EiInput::new().await {
                Ok(ei_input) => {
                    tracing::info!("Input injection active (libei)");
                    server::LiveInputHandler::new(ei_input)
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize input injection: {e}");
                    tracing::warn!("Input events will be logged but not injected");
                    let rdp_server = server::build_view_only_server(
                        cfg.bind, tls_ctx, auth, live_display, make_cliprdr(), make_sound(),
                    );
                    let _capture = capture_handle;
                    return run_with_shutdown(rdp_server, dbus_cmd_rx).await;
                }
            };

            let rdp_server = server::build_live_server(
                cfg.bind, tls_ctx, auth, live_display, input_handler,
                make_cliprdr(), make_sound(), Some(Box::new(egfx_factory)),
            );
            // Set the event sender so the EGFX controller can push
            // H.264 frames proactively via ServerEvent::DvcOutput.
            egfx_event_setter.set_event_sender(rdp_server.event_sender().clone());
            let _capture = capture_handle;
            run_with_shutdown(rdp_server, dbus_cmd_rx).await
        }
        Err(e) => {
            tracing::warn!("Failed to start screen capture: {e:#}");
            tracing::info!("Falling back to static blue screen display");
            let (egfx_factory, _egfx_controller, egfx_event_setter) =
                egfx::create_egfx(1920, 1080);
            let rdp_server =
                server::build_server(cfg.bind, tls_ctx, auth, make_cliprdr(), make_sound(),
                    Some(Box::new(egfx_factory)));
            egfx_event_setter.set_event_sender(rdp_server.event_sender().clone());
            run_with_shutdown(rdp_server, dbus_cmd_rx).await
        }
    }
}

/// Run the RDP server with graceful shutdown on `SIGINT` / `SIGTERM` or
/// D-Bus commands.
async fn run_with_shutdown(
    mut server: ironrdp_server::RdpServer,
    dbus_cmd_rx: &mut tokio::sync::mpsc::Receiver<rdp_dbus::server::DaemonCommand>,
) -> Result<ShutdownReason> {
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("failed to register SIGTERM handler")?;

    tokio::select! {
        result = server.run() => {
            result.context("RDP server error")?;
            Ok(ShutdownReason::Stop)
        }
        result = tokio::signal::ctrl_c() => {
            result.context("failed to listen for SIGINT")?;
            tracing::info!("Received SIGINT, shutting down");
            Ok(ShutdownReason::Signal)
        }
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, shutting down");
            Ok(ShutdownReason::Signal)
        }
        cmd = dbus_cmd_rx.recv() => {
            match cmd {
                Some(rdp_dbus::server::DaemonCommand::Reload) => {
                    tracing::info!("D-Bus: reload requested");
                    Ok(ShutdownReason::Reload)
                }
                Some(rdp_dbus::server::DaemonCommand::Stop) | None => {
                    tracing::info!("D-Bus: stop requested");
                    Ok(ShutdownReason::Stop)
                }
            }
        }
    }
}

/// Returns `true` if the address is a loopback address (`127.0.0.1`, `::1`).
fn is_localhost(ip: std::net::IpAddr) -> bool {
    ip.is_loopback()
}

/// Path to the `ScreenCast` portal restore token file.
///
/// Saved under `$XDG_RUNTIME_DIR/cosmic-rdp-server/restore_token` so it
/// persists across service restarts within the same login session but is
/// cleared on logout.
fn restore_token_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| PathBuf::from(dir).join("cosmic-rdp-server").join("restore_token"))
}

/// Load a previously saved `ScreenCast` portal restore token.
fn load_restore_token() -> Option<String> {
    let path = restore_token_path()?;
    let token = std::fs::read_to_string(&path).ok()?;
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    tracing::info!(path = %path.display(), "Loaded ScreenCast restore token");
    Some(token.to_string())
}

/// Run a background H.264 encoding loop for static display testing.
///
/// Generates a color test pattern (RGBW quadrants) and continuously
/// encodes it as H.264 frames through the EGFX channel. This allows
/// testing the full encode→decode color pipeline without live capture.
///
/// The client will first see the blue bitmap from [`StaticDisplay`], then
/// switch to the RGBW test pattern once EGFX AVC420 negotiates (~1-2s).
async fn static_egfx_task(
    controller: egfx::EgfxController,
    width: u16,
    height: u16,
) {
    // Wait for EGFX to become ready (client must negotiate AVC420).
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if controller.is_ready() && controller.supports_avc420() {
            break;
        }
    }

    tracing::info!(width, height, "Static EGFX: channel ready, starting H.264 test pattern");

    let config = EncoderConfig {
        width: u32::from(width),
        height: u32::from(height),
        ..EncoderConfig::default()
    };

    let mut encoder = match GstEncoder::new(&config) {
        Ok(enc) => {
            tracing::info!(encoder_type = %enc.encoder_type(), "Static EGFX: encoder created");
            enc
        }
        Err(e) => {
            tracing::error!("Static EGFX: failed to create encoder: {e}");
            return;
        }
    };

    let frame_data = create_color_test_pattern(width, height);
    let mut timestamp_ms: u32 = 0;
    let mut sent_count: u32 = 0;

    loop {
        match encoder.encode_frame(&frame_data) {
            Ok(Some(h264_frame)) => {
                if controller.send_frame(&h264_frame.data, width, height, timestamp_ms) {
                    sent_count += 1;
                    if sent_count <= 5 {
                        tracing::info!(
                            sent_count,
                            h264_size = h264_frame.data.len(),
                            is_keyframe = h264_frame.is_keyframe,
                            "Static EGFX: sent H.264 frame"
                        );
                    }
                }
                timestamp_ms = timestamp_ms.wrapping_add(500);
            }
            Ok(None) => {
                // Encoder is buffering, try again quickly.
            }
            Err(e) => {
                tracing::warn!("Static EGFX: encode error: {e}");
                return;
            }
        }
        // 2 fps is plenty for a static test pattern.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Create a color test pattern with RGBW quadrants for visual color verification.
///
/// Layout (BGRA pixel data):
/// ```text
/// ┌──────────┬──────────┐
/// │  RED     │  GREEN   │
/// │          │          │
/// ├──────────┼──────────┤
/// │  BLUE    │  WHITE   │
/// │          │          │
/// └──────────┴──────────┘
/// ```
///
/// If any color channel is swapped in the H.264 encode→decode pipeline, it
/// will be immediately visible (e.g. red quadrant appearing blue).
fn create_color_test_pattern(width: u16, height: u16) -> Vec<u8> {
    let w = usize::from(width);
    let h = usize::from(height);
    let bpp = 4;
    let mut data = vec![0u8; w * h * bpp];
    let half_w = w / 2;
    let half_h = h / 2;

    for y in 0..h {
        for x in 0..w {
            let offset = (y * w + x) * bpp;
            // BGRx format: [B, G, R, x]
            let pixel: [u8; 4] = if y < half_h {
                if x < half_w {
                    [0x00, 0x00, 0xFF, 0xFF] // Red (B=0, G=0, R=255)
                } else {
                    [0x00, 0xFF, 0x00, 0xFF] // Green (B=0, G=255, R=0)
                }
            } else if x < half_w {
                [0xFF, 0x00, 0x00, 0xFF] // Blue (B=255, G=0, R=0)
            } else {
                [0xFF, 0xFF, 0xFF, 0xFF] // White
            };
            data[offset..offset + bpp].copy_from_slice(&pixel);
        }
    }

    data
}

/// Save the `ScreenCast` portal restore token for future service restarts.
fn save_restore_token(token: &str) {
    let Some(path) = restore_token_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!("Failed to create restore token dir: {e}");
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, token) {
        tracing::warn!("Failed to save restore token: {e}");
    } else {
        tracing::info!(path = %path.display(), "Saved ScreenCast restore token");
    }
}
