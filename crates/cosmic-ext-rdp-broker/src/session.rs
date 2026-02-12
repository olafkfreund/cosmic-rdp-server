use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rdp_dbus::types::SessionState;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Persisted session entry (written to JSON state file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub username: String,
    pub port: u16,
    pub pid: u32,
    pub state: SessionStateSerde,
    pub created_at: i64,
    pub client_addr: String,
    /// systemd transient unit name (for cleanup).
    pub unit_name: String,
}

/// Serializable session state (mirrors `rdp_dbus::types::SessionState`
/// but with string-based serde for the JSON state file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStateSerde {
    Starting,
    Active,
    Idle,
    Stopping,
}

impl From<SessionStateSerde> for SessionState {
    fn from(s: SessionStateSerde) -> Self {
        match s {
            SessionStateSerde::Starting => Self::Starting,
            SessionStateSerde::Active => Self::Active,
            SessionStateSerde::Idle => Self::Idle,
            SessionStateSerde::Stopping => Self::Stopping,
        }
    }
}

impl From<SessionState> for SessionStateSerde {
    fn from(s: SessionState) -> Self {
        match s {
            SessionState::Starting => Self::Starting,
            SessionState::Active => Self::Active,
            SessionState::Idle => Self::Idle,
            SessionState::Stopping => Self::Stopping,
        }
    }
}

/// Thread-safe session registry.
#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

struct RegistryInner {
    sessions: HashMap<String, SessionEntry>,
    port_range_start: u16,
    port_range_end: u16,
    max_sessions: usize,
    state_file: std::path::PathBuf,
}

impl SessionRegistry {
    /// Create a new session registry.
    #[must_use]
    pub fn new(port_range_start: u16, port_range_end: u16, max_sessions: usize, state_file: std::path::PathBuf) -> Self {
        Self {
            inner: Arc::new(RwLock::new(RegistryInner {
                sessions: HashMap::new(),
                port_range_start,
                port_range_end,
                max_sessions,
                state_file,
            })),
        }
    }

    /// Load sessions from the persisted state file.
    ///
    /// Verifies each session's PID is still alive and removes stale entries.
    pub async fn load_state(&self) -> Result<()> {
        let state_file = self.inner.read().await.state_file.clone();
        if !state_file.exists() {
            return Ok(());
        }

        let contents = std::fs::read_to_string(&state_file)
            .with_context(|| format!("failed to read state file: {}", state_file.display()))?;

        let entries: Vec<SessionEntry> = serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse state file: {}", state_file.display()))?;

        let mut inner = self.inner.write().await;
        for entry in entries {
            // Verify PID is still alive.
            if is_pid_alive(entry.pid) {
                tracing::info!(
                    username = %entry.username,
                    port = entry.port,
                    pid = entry.pid,
                    "Restored session from state file"
                );
                inner.sessions.insert(entry.username.clone(), entry);
            } else {
                tracing::info!(
                    username = %entry.username,
                    pid = entry.pid,
                    "Stale session removed (PID not alive)"
                );
            }
        }

        Ok(())
    }

    /// Persist current sessions to the state file.
    pub async fn save_state(&self) -> Result<()> {
        let inner = self.inner.read().await;
        let entries: Vec<&SessionEntry> = inner.sessions.values().collect();
        let contents = serde_json::to_string_pretty(&entries)
            .context("failed to serialize sessions")?;

        if let Some(parent) = inner.state_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create state dir: {}", parent.display()))?;
        }

        // Atomic write via temp file + rename.
        let tmp_path = inner.state_file.with_extension("json.tmp");
        std::fs::write(&tmp_path, &contents)
            .with_context(|| format!("failed to write temp state: {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &inner.state_file)
            .with_context(|| format!("failed to rename state file: {}", inner.state_file.display()))?;

        Ok(())
    }

    /// Look up a session by username.
    pub async fn get(&self, username: &str) -> Option<SessionEntry> {
        self.inner.read().await.sessions.get(username).cloned()
    }

    /// Allocate a port for a new session.
    ///
    /// # Errors
    ///
    /// Returns an error if no ports are available or the max session limit
    /// is reached.
    pub async fn allocate_port(&self) -> Result<u16> {
        let inner = self.inner.read().await;

        if inner.sessions.len() >= inner.max_sessions {
            bail!(
                "maximum session limit ({}) reached",
                inner.max_sessions
            );
        }

        let used_ports: std::collections::HashSet<u16> =
            inner.sessions.values().map(|s| s.port).collect();

        for port in inner.port_range_start..=inner.port_range_end {
            if !used_ports.contains(&port) {
                return Ok(port);
            }
        }

        bail!(
            "no ports available in range {}-{}",
            inner.port_range_start,
            inner.port_range_end
        );
    }

    /// Register a new session.
    pub async fn insert(&self, entry: SessionEntry) {
        let mut inner = self.inner.write().await;
        inner.sessions.insert(entry.username.clone(), entry);
    }

    /// Update the state of a session.
    pub async fn set_state(&self, username: &str, state: SessionStateSerde) {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.sessions.get_mut(username) {
            entry.state = state;
        }
    }

    /// Update the client address of a session.
    pub async fn set_client_addr(&self, username: &str, addr: &str) {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.sessions.get_mut(username) {
            entry.client_addr = addr.to_string();
        }
    }

    /// Remove a session from the registry.
    pub async fn remove(&self, username: &str) -> Option<SessionEntry> {
        let mut inner = self.inner.write().await;
        inner.sessions.remove(username)
    }

    /// List all sessions.
    pub async fn list(&self) -> Vec<SessionEntry> {
        self.inner.read().await.sessions.values().cloned().collect()
    }

    /// Number of active sessions.
    pub async fn count(&self) -> usize {
        self.inner.read().await.sessions.len()
    }

    /// Find idle sessions that have exceeded the given timeout.
    pub async fn idle_sessions(&self, timeout_secs: u64) -> Vec<String> {
        let now = now_unix();
        let inner = self.inner.read().await;
        inner
            .sessions
            .values()
            .filter(|s| {
                s.state == SessionStateSerde::Idle
                    && now.saturating_sub(s.created_at).try_into().unwrap_or(0) > timeout_secs
            })
            .map(|s| s.username.clone())
            .collect()
    }
}

/// Check if a process with the given PID is still alive.
fn is_pid_alive(pid: u32) -> bool {
    // Send signal 0 to check if process exists.
    #[allow(clippy::cast_possible_wrap)]
    let pid = nix::unistd::Pid::from_raw(pid as i32);
    nix::sys::signal::kill(pid, None).is_ok()
}

/// Get current Unix timestamp in seconds.
#[allow(clippy::cast_possible_wrap)]
#[must_use]
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
