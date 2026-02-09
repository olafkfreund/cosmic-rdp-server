use std::net::SocketAddr;

use cosmic::app::Core;
use cosmic::iced::Length;
use cosmic::widget::{self, nav_bar};
use cosmic::{Application, Element};

use crate::config;
use crate::fl;
use crate::message::{Message, Page};
use crate::pages::{display, features};

/// COSMIC RDP Settings application.
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    core: Core,
    /// Current nav page.
    current_page: Page,
    /// Nav model.
    nav: nav_bar::Model,

    // -- Server status (from D-Bus) --
    server_running: bool,
    active_connections: u32,
    bound_address: String,

    // -- General settings --
    bind_address: String,
    port: String,
    static_display: bool,

    // -- Security settings --
    cert_path: String,
    key_path: String,
    nla_enable: bool,
    nla_username: String,
    nla_password: String,
    nla_domain: String,

    // -- Display settings --
    fps: String,
    buffer_capacity: String,
    multi_monitor: bool,
    encoder_idx: usize,
    preset: String,
    bitrate_mbps: String,

    // -- Features settings --
    clipboard_enable: bool,
    audio_enable: bool,
    sample_rate_idx: usize,
    channels_idx: usize,

    // -- Dropdown labels (owned for lifetime) --
    encoder_labels: Vec<String>,
    sample_rate_labels: Vec<String>,
    channel_labels: Vec<String>,
}

impl App {
    /// Apply the loaded configuration to the UI state.
    fn apply_config(&mut self, cfg: &rdp_dbus::config::ServerConfig) {
        let addr: SocketAddr = cfg.bind;
        self.bind_address = addr.ip().to_string();
        self.port = addr.port().to_string();
        self.static_display = cfg.static_display;

        self.cert_path = cfg
            .cert_path
            .as_ref()
            .map_or_else(String::new, |p| p.display().to_string());
        self.key_path = cfg
            .key_path
            .as_ref()
            .map_or_else(String::new, |p| p.display().to_string());
        self.nla_enable = cfg.auth.enable;
        self.nla_username.clone_from(&cfg.auth.username);
        self.nla_password.clone_from(&cfg.auth.password);
        self.nla_domain = cfg.auth.domain.clone().unwrap_or_default();

        self.fps = cfg.capture.fps.to_string();
        self.buffer_capacity = cfg.capture.channel_capacity.to_string();
        self.multi_monitor = cfg.capture.multi_monitor;

        self.encoder_idx = display::ENCODER_OPTIONS
            .iter()
            .position(|&e| e == cfg.encode.encoder)
            .unwrap_or(0);
        self.preset.clone_from(&cfg.encode.preset);
        self.bitrate_mbps = format!("{:.1}", f64::from(cfg.encode.bitrate) / 1_000_000.0);

        self.clipboard_enable = cfg.clipboard.enable;
        self.audio_enable = cfg.audio.enable;
        self.sample_rate_idx = features::SAMPLE_RATE_OPTIONS
            .iter()
            .position(|&r| r == cfg.audio.sample_rate)
            .unwrap_or(0);
        self.channels_idx = features::CHANNEL_OPTIONS
            .iter()
            .position(|&c| c == cfg.audio.channels)
            .unwrap_or(1);
    }

    /// Build a [`ServerConfig`] from the current UI state.
    fn build_config(&self) -> rdp_dbus::config::ServerConfig {
        use std::path::PathBuf;

        let bind: SocketAddr = format!("{}:{}", self.bind_address, self.port)
            .parse()
            .unwrap_or_else(|_| "0.0.0.0:3389".parse().expect("valid fallback"));

        let cert_path = if self.cert_path.is_empty() {
            None
        } else {
            Some(PathBuf::from(&self.cert_path))
        };
        let key_path = if self.key_path.is_empty() {
            None
        } else {
            Some(PathBuf::from(&self.key_path))
        };
        let domain = if self.nla_domain.is_empty() {
            None
        } else {
            Some(self.nla_domain.clone())
        };

        let encoder = display::ENCODER_OPTIONS
            .get(self.encoder_idx)
            .copied()
            .unwrap_or("auto")
            .to_string();

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bitrate = self
            .bitrate_mbps
            .parse::<f64>()
            .map(|mbps| (mbps * 1_000_000.0) as u32)
            .unwrap_or(10_000_000);

        let sample_rate = features::SAMPLE_RATE_OPTIONS
            .get(self.sample_rate_idx)
            .copied()
            .unwrap_or(44100);
        let channels = features::CHANNEL_OPTIONS
            .get(self.channels_idx)
            .copied()
            .unwrap_or(2);

        rdp_dbus::config::ServerConfig {
            bind,
            cert_path,
            key_path,
            static_display: self.static_display,
            auth: rdp_dbus::config::AuthConfig {
                enable: self.nla_enable,
                username: self.nla_username.clone(),
                password: self.nla_password.clone(),
                domain,
            },
            capture: rdp_dbus::config::CaptureConfig {
                fps: self.fps.parse().unwrap_or(30),
                channel_capacity: self.buffer_capacity.parse().unwrap_or(4),
                multi_monitor: self.multi_monitor,
            },
            encode: rdp_dbus::config::EncodeConfig {
                encoder,
                preset: self.preset.clone(),
                bitrate,
            },
            clipboard: rdp_dbus::config::ClipboardConfig {
                enable: self.clipboard_enable,
            },
            audio: rdp_dbus::config::AudioConfig {
                enable: self.audio_enable,
                sample_rate,
                channels,
            },
        }
    }
}

impl Application for App {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "com.system76.CosmicRdpSettings";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, cosmic::app::Task<Message>) {
        let mut nav = nav_bar::Model::default();
        nav.insert()
            .text(fl!("nav-general"))
            .data::<Page>(Page::General)
            .activate();
        nav.insert()
            .text(fl!("nav-security"))
            .data::<Page>(Page::Security);
        nav.insert()
            .text(fl!("nav-display"))
            .data::<Page>(Page::Display);
        nav.insert()
            .text(fl!("nav-features"))
            .data::<Page>(Page::Features);

        let app = App {
            core,
            current_page: Page::General,
            nav,
            server_running: false,
            active_connections: 0,
            bound_address: String::new(),
            bind_address: "0.0.0.0".to_string(),
            port: "3389".to_string(),
            static_display: false,
            cert_path: String::new(),
            key_path: String::new(),
            nla_enable: false,
            nla_username: String::new(),
            nla_password: String::new(),
            nla_domain: String::new(),
            fps: "30".to_string(),
            buffer_capacity: "4".to_string(),
            multi_monitor: false,
            encoder_idx: 0,
            preset: "ultrafast".to_string(),
            bitrate_mbps: "10.0".to_string(),
            clipboard_enable: true,
            audio_enable: true,
            sample_rate_idx: 0,
            channels_idx: 1,
            encoder_labels: vec![
                fl!("display-encoder-auto"),
                fl!("display-encoder-vaapi"),
                fl!("display-encoder-nvenc"),
                fl!("display-encoder-software"),
            ],
            sample_rate_labels: vec!["44100 Hz".to_string(), "48000 Hz".to_string()],
            channel_labels: vec![fl!("features-channels-mono"), fl!("features-channels-stereo")],
        };

        let task = cosmic::task::future(async {
            match config::load() {
                Ok(cfg) => Message::ConfigLoaded(Box::new(cfg)),
                Err(e) => Message::Error(e.to_string()),
            }
        });

        (app, task)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> cosmic::app::Task<Message> {
        if let Some(page) = self.nav.data::<Page>(id) {
            self.current_page = *page;
        }
        cosmic::task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let page_content: Element<'_, Message> = match self.current_page {
            Page::General => crate::pages::general::view(
                &self.bind_address,
                &self.port,
                self.static_display,
                self.server_running,
                self.active_connections,
                &self.bound_address,
            ),
            Page::Security => crate::pages::security::view(
                &self.cert_path,
                &self.key_path,
                self.nla_enable,
                &self.nla_username,
                &self.nla_password,
                &self.nla_domain,
            ),
            Page::Display => crate::pages::display::view(
                &self.fps,
                &self.buffer_capacity,
                self.multi_monitor,
                self.encoder_idx,
                &self.preset,
                &self.bitrate_mbps,
                &self.encoder_labels,
            ),
            Page::Features => crate::pages::features::view(
                self.clipboard_enable,
                self.audio_enable,
                self.sample_rate_idx,
                self.channels_idx,
                &self.sample_rate_labels,
                &self.channel_labels,
            ),
        };

        widget::container(page_content)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(24)
            .into()
    }

    fn update(&mut self, message: Self::Message) -> cosmic::app::Task<Message> {
        match message {
            // General
            Message::BindAddress(v) => self.bind_address = v,
            Message::Port(v) => self.port = v,
            Message::StaticDisplay(v) => self.static_display = v,

            // Security
            Message::CertPath(v) => self.cert_path = v,
            Message::KeyPath(v) => self.key_path = v,
            Message::NlaEnable(v) => self.nla_enable = v,
            Message::NlaUsername(v) => self.nla_username = v,
            Message::NlaPassword(v) => self.nla_password = v,
            Message::NlaDomain(v) => self.nla_domain = v,

            // Display
            Message::Fps(v) => self.fps = v,
            Message::BufferCapacity(v) => self.buffer_capacity = v,
            Message::MultiMonitor(v) => self.multi_monitor = v,
            Message::Encoder(idx) => self.encoder_idx = idx,
            Message::Preset(v) => self.preset = v,
            Message::Bitrate(v) => self.bitrate_mbps = v,

            // Features
            Message::ClipboardEnable(v) => self.clipboard_enable = v,
            Message::AudioEnable(v) => self.audio_enable = v,
            Message::SampleRate(idx) => self.sample_rate_idx = idx,
            Message::Channels(idx) => self.channels_idx = idx,

            // Apply: save config + D-Bus reload
            Message::Apply => {
                let cfg = self.build_config();
                return cosmic::task::future(async move {
                    if let Err(e) = config::save(&cfg) {
                        return Message::Error(e.to_string());
                    }
                    if let Err(e) = dbus_reload().await {
                        tracing::warn!("D-Bus reload failed: {e}");
                    }
                    Message::ConfigSaved
                });
            }

            // Reset: reload config from disk
            Message::Reset => {
                return cosmic::task::future(async {
                    match config::load() {
                        Ok(cfg) => Message::ConfigLoaded(Box::new(cfg)),
                        Err(e) => Message::Error(e.to_string()),
                    }
                });
            }

            // Toggle server via D-Bus
            Message::ToggleServer(enable) => {
                if enable {
                    tracing::info!("Server start requested - use systemctl or CLI");
                } else {
                    return cosmic::task::future(async {
                        match dbus_stop().await {
                            Ok(()) => Message::StopSent,
                            Err(e) => Message::Error(e.to_string()),
                        }
                    });
                }
            }

            // D-Bus status update
            Message::StatusUpdate {
                running,
                connections,
                address,
            } => {
                self.server_running = running;
                self.active_connections = connections;
                self.bound_address = address;
            }
            Message::DbusUnavailable => {
                self.server_running = false;
                self.active_connections = 0;
                self.bound_address.clear();
            }

            // Async results
            Message::ConfigLoaded(cfg) => {
                self.apply_config(&cfg);
            }
            Message::ConfigSaved => {
                tracing::info!("Configuration saved and reload sent");
            }
            Message::StopSent => {}
            Message::Error(e) => {
                tracing::error!("Settings error: {e}");
            }

            // Poll D-Bus for server status
            Message::PollStatus => {
                return cosmic::task::future(async {
                    match dbus_poll_status().await {
                        Ok((running, connections, address)) => Message::StatusUpdate {
                            running,
                            connections,
                            address,
                        },
                        Err(_) => Message::DbusUnavailable,
                    }
                });
            }
        }

        cosmic::task::none()
    }

    fn subscription(&self) -> cosmic::iced::Subscription<Self::Message> {
        cosmic::iced::time::every(std::time::Duration::from_secs(2)).map(|_| {
            // Poll D-Bus status in a blocking-safe way via message.
            // The actual async call happens in update() below.
            Message::PollStatus
        })
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        vec![widget::text::title4(fl!("app-title")).into()]
    }
}

/// Send a D-Bus `Reload()` to the daemon.
async fn dbus_reload() -> anyhow::Result<()> {
    let connection = zbus::Connection::session().await?;
    let proxy = rdp_dbus::client::RdpServerProxy::new(&connection).await?;
    proxy.reload().await?;
    Ok(())
}

/// Send a D-Bus `Stop()` to the daemon.
async fn dbus_stop() -> anyhow::Result<()> {
    let connection = zbus::Connection::session().await?;
    let proxy = rdp_dbus::client::RdpServerProxy::new(&connection).await?;
    proxy.stop().await?;
    Ok(())
}

/// Poll D-Bus for the current server status.
async fn dbus_poll_status() -> anyhow::Result<(bool, u32, String)> {
    let connection = zbus::Connection::session().await?;
    let proxy = rdp_dbus::client::RdpServerProxy::new(&connection).await?;
    let running = proxy.running().await?;
    let connections = proxy.active_connections().await?;
    let address = proxy.bound_address().await?;
    Ok((running, connections, address))
}
