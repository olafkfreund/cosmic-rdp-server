/// All messages that the settings application can handle.
#[derive(Debug, Clone)]
pub enum Message {
    // -- General page --
    /// Toggle the server on/off via D-Bus.
    ToggleServer(bool),
    /// Bind address changed.
    BindAddress(String),
    /// Port changed.
    Port(String),
    /// Static display toggle.
    StaticDisplay(bool),

    // -- Security page --
    /// TLS certificate path changed.
    CertPath(String),
    /// TLS key path changed.
    KeyPath(String),
    /// NLA enable toggle.
    NlaEnable(bool),
    /// NLA username changed.
    NlaUsername(String),
    /// NLA password changed.
    NlaPassword(String),
    /// NLA domain changed.
    NlaDomain(String),

    // -- Display page --
    /// FPS changed.
    Fps(String),
    /// Buffer capacity changed.
    BufferCapacity(String),
    /// Multi-monitor toggle.
    MultiMonitor(bool),
    /// Encoder selection changed.
    Encoder(usize),
    /// Encoding preset changed.
    Preset(String),
    /// Bitrate changed (Mbps input).
    Bitrate(String),

    // -- Features page --
    /// Clipboard toggle.
    ClipboardEnable(bool),
    /// Audio toggle.
    AudioEnable(bool),
    /// Audio sample rate selection.
    SampleRate(usize),
    /// Audio channels selection.
    Channels(usize),

    // -- Actions --
    /// Apply settings: write TOML and D-Bus reload.
    Apply,
    /// Reset settings from disk.
    Reset,

    // -- D-Bus status --
    /// Server status update from D-Bus polling.
    StatusUpdate {
        running: bool,
        connections: u32,
        address: String,
    },
    /// D-Bus is not available.
    DbusUnavailable,

    // -- Async results --
    /// Config loaded from disk.
    ConfigLoaded(Box<rdp_dbus::config::ServerConfig>),
    /// Config saved successfully.
    ConfigSaved,
    /// An error occurred.
    Error(String),
    /// D-Bus stop sent.
    StopSent,

    /// Poll D-Bus status (fired by subscription timer).
    PollStatus,
}

/// Pages in the settings navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Security,
    Display,
    Features,
}
