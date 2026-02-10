//! Direct `reis` (libei) backend for input injection.
//!
//! Uses the `reis` crate to speak the libei protocol directly,
//! connecting via the `RemoteDesktop` XDG portal (`ashpd`).
//! This avoids the X11 fallback logic and panic-prone initialization
//! path that the `enigo` crate previously used.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::time::SystemTime;

use reis::ei;
use reis::handshake::ei_handshake_blocking;
use reis::PendingRequestResult;

use crate::keymap::rdp_scancode_to_evdev;

/// Evdev keycodes for lock keys.
const KEY_CAPSLOCK: u16 = 66;
const KEY_NUMLOCK: u16 = 77;
const KEY_SCROLLLOCK: u16 = 78;

/// Shadow state for lock key indicators (Caps Lock, Num Lock, Scroll Lock).
///
/// The RDP client periodically sends `Synchronize` events with the current
/// lock key state. We track our own shadow state based on injected key events
/// and toggle mismatches when a `Synchronize` arrives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockState {
    pub caps_lock: bool,
    pub num_lock: bool,
    pub scroll_lock: bool,
}

impl LockState {
    /// Return which lock keys need toggling to reach the `target` state.
    #[must_use]
    pub fn locks_to_toggle(&self, target: &Self) -> Vec<u16> {
        let mut keys = Vec::new();
        if self.caps_lock != target.caps_lock {
            keys.push(KEY_CAPSLOCK);
        }
        if self.num_lock != target.num_lock {
            keys.push(KEY_NUMLOCK);
        }
        if self.scroll_lock != target.scroll_lock {
            keys.push(KEY_SCROLLLOCK);
        }
        keys
    }

    /// Toggle the shadow state for a lock key that was just pressed.
    fn toggle_on_press(&mut self, evdev: u16) {
        match evdev {
            KEY_CAPSLOCK => self.caps_lock = !self.caps_lock,
            KEY_NUMLOCK => self.num_lock = !self.num_lock,
            KEY_SCROLLLOCK => self.scroll_lock = !self.scroll_lock,
            _ => {}
        }
    }
}

/// Linux input event codes for mouse buttons.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const BTN_FORWARD: u32 = 0x115;
const BTN_BACK: u32 = 0x116;

/// Mouse button identifiers matching the RDP protocol.
#[derive(Debug, Clone, Copy)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl MouseButton {
    /// Convert to the Linux evdev button code.
    const fn to_linux_code(self) -> u32 {
        match self {
            Self::Left => BTN_LEFT,
            Self::Right => BTN_RIGHT,
            Self::Middle => BTN_MIDDLE,
            Self::Back => BTN_BACK,
            Self::Forward => BTN_FORWARD,
        }
    }
}

/// Data collected during device enumeration.
#[derive(Default)]
struct DeviceData {
    interfaces: HashMap<String, reis::Object>,
}

impl DeviceData {
    fn interface<T: reis::Interface>(&self) -> Option<T> {
        self.interfaces.get(T::NAME)?.clone().downcast()
    }
}

/// Input injector backed by `reis` (direct libei protocol).
///
/// Connects to the COSMIC compositor via the `RemoteDesktop` XDG
/// portal and injects keyboard, mouse, and scroll events over the
/// libei wire protocol.
pub struct EiInput {
    context: ei::Context,
    device: ei::Device,
    keyboard: Option<ei::Keyboard>,
    pointer: Option<ei::Pointer>,
    pointer_abs: Option<ei::PointerAbsolute>,
    button: Option<ei::Button>,
    scroll: Option<ei::Scroll>,
    serial: u32,
    sequence: u32,
    emulating: bool,
    /// Shadow state for lock key indicators, updated on every injected
    /// key press and compared against `Synchronize` events.
    lock_state: LockState,
}

impl EiInput {
    /// Create a new input injector.
    ///
    /// Connects to the compositor via the `RemoteDesktop` XDG portal
    /// to obtain an EIS socket, then performs the libei handshake and
    /// discovers input capabilities.
    ///
    /// # Errors
    ///
    /// Returns [`InputError::Init`] if the portal session cannot be
    /// established, the handshake fails, or no input device is found.
    pub async fn new() -> Result<Self, InputError> {
        let (context, serial) = setup_ei_context().await?;
        discover_devices(context, serial)
    }

    /// Get the current timestamp in microseconds for frame events.
    #[allow(clippy::cast_possible_truncation)]
    fn timestamp_us() -> u64 {
        SystemTime::UNIX_EPOCH
            .elapsed()
            .map_or(0, |d| d.as_micros() as u64)
    }

    /// Ensure we are in emulating mode before sending events.
    fn ensure_emulating(&mut self) {
        if !self.emulating {
            self.device.start_emulating(self.serial, self.sequence);
            self.sequence += 1;
            self.emulating = true;
            let _ = self.context.flush();
        }
    }

    /// Send a frame event and flush the context.
    fn frame_and_flush(&self) {
        let ts = Self::timestamp_us();
        self.device.frame(self.serial, ts);
        let _ = self.context.flush();
    }

    /// Inject a keyboard key press.
    ///
    /// Converts the RDP XT scancode to an evdev keycode and sends a press event.
    /// Also updates the shadow lock key state when a lock key is pressed.
    pub fn key_press(&mut self, code: u8, extended: bool) {
        if self.keyboard.is_none() {
            tracing::debug!("No keyboard capability, ignoring key press");
            return;
        }
        let Some(evdev) = rdp_scancode_to_evdev(code, extended) else {
            tracing::debug!(code, extended, "Unmapped scancode (press)");
            return;
        };
        self.ensure_emulating();
        // ei protocol uses evdev keycodes minus 8 (XKB offset)
        if let Some(ref keyboard) = self.keyboard {
            keyboard.key(u32::from(evdev) - 8, ei::keyboard::KeyState::Press);
        }
        self.lock_state.toggle_on_press(evdev);
        self.frame_and_flush();
    }

    /// Synchronize lock key state with the RDP client.
    ///
    /// Compares the client's reported lock state against our shadow state
    /// and injects press+release events for any mismatched lock keys.
    pub fn synchronize_locks(&mut self, caps: bool, num: bool, scroll: bool) {
        let target = LockState {
            caps_lock: caps,
            num_lock: num,
            scroll_lock: scroll,
        };

        let to_toggle = self.lock_state.locks_to_toggle(&target);
        if to_toggle.is_empty() {
            tracing::trace!("Lock state already in sync");
            return;
        }

        tracing::debug!(
            ?target,
            current = ?self.lock_state,
            count = to_toggle.len(),
            "Synchronizing lock keys"
        );

        for evdev in to_toggle {
            self.toggle_lock_key(evdev);
        }
    }

    /// Toggle a lock key by injecting press + release with separate frames.
    fn toggle_lock_key(&mut self, evdev: u16) {
        if self.keyboard.is_none() {
            return;
        }
        self.ensure_emulating();
        let xkb = u32::from(evdev) - 8;

        // Press
        if let Some(ref keyboard) = self.keyboard {
            keyboard.key(xkb, ei::keyboard::KeyState::Press);
        }
        self.frame_and_flush();

        // Release
        if let Some(ref keyboard) = self.keyboard {
            keyboard.key(xkb, ei::keyboard::KeyState::Released);
        }
        self.lock_state.toggle_on_press(evdev);
        self.frame_and_flush();
    }

    /// Inject a keyboard key release.
    ///
    /// Converts the RDP XT scancode to an evdev keycode and sends a release event.
    pub fn key_release(&mut self, code: u8, extended: bool) {
        if self.keyboard.is_none() {
            tracing::debug!("No keyboard capability, ignoring key release");
            return;
        }
        let Some(evdev) = rdp_scancode_to_evdev(code, extended) else {
            tracing::debug!(code, extended, "Unmapped scancode (release)");
            return;
        };
        self.ensure_emulating();
        if let Some(ref keyboard) = self.keyboard {
            keyboard.key(u32::from(evdev) - 8, ei::keyboard::KeyState::Released);
        }
        self.frame_and_flush();
    }

    /// Move the mouse to absolute coordinates.
    ///
    /// Coordinates are in desktop pixels as reported by the RDP client.
    pub fn mouse_move(&mut self, x: u16, y: u16) {
        if self.pointer_abs.is_none() {
            tracing::debug!("No absolute pointer capability, ignoring mouse move");
            return;
        }
        self.ensure_emulating();
        if let Some(ref pointer_abs) = self.pointer_abs {
            pointer_abs.motion_absolute(f32::from(x), f32::from(y));
        }
        self.frame_and_flush();
    }

    /// Move the mouse by a relative offset.
    pub fn mouse_rel_move(&mut self, x: i32, y: i32) {
        if self.pointer.is_none() {
            tracing::debug!("No relative pointer capability, ignoring rel move");
            return;
        }
        self.ensure_emulating();
        if let Some(ref pointer) = self.pointer {
            #[allow(clippy::cast_precision_loss)]
            pointer.motion_relative(x as f32, y as f32);
        }
        self.frame_and_flush();
    }

    /// Press or release a mouse button.
    pub fn mouse_button(&mut self, btn: MouseButton, pressed: bool) {
        if self.button.is_none() {
            tracing::debug!("No button capability, ignoring mouse button");
            return;
        }
        let state = if pressed {
            ei::button::ButtonState::Press
        } else {
            ei::button::ButtonState::Released
        };
        self.ensure_emulating();
        if let Some(ref button) = self.button {
            button.button(btn.to_linux_code(), state);
        }
        self.frame_and_flush();
    }

    /// Scroll vertically.
    ///
    /// Positive values scroll down, negative scroll up (matching RDP convention).
    pub fn scroll_vertical(&mut self, value: i32) {
        if self.scroll.is_none() {
            tracing::debug!("No scroll capability, ignoring vertical scroll");
            return;
        }
        self.ensure_emulating();
        if let Some(ref scroll) = self.scroll {
            #[allow(clippy::cast_precision_loss)]
            scroll.scroll(0.0, value as f32);
        }
        self.frame_and_flush();
    }

    /// Scroll with explicit x/y amounts.
    pub fn scroll(&mut self, x: i32, y: i32) {
        if self.scroll.is_none() {
            tracing::debug!("No scroll capability, ignoring scroll");
            return;
        }
        if x == 0 && y == 0 {
            return;
        }
        self.ensure_emulating();
        if let Some(ref scroll) = self.scroll {
            #[allow(clippy::cast_precision_loss)]
            scroll.scroll(x as f32, y as f32);
        }
        self.frame_and_flush();
    }
}

impl Drop for EiInput {
    fn drop(&mut self) {
        if self.emulating {
            self.device.stop_emulating(self.serial);
            let _ = self.context.flush();
        }
    }
}

/// Establish an EIS connection via the `RemoteDesktop` portal and
/// perform the libei handshake.
///
/// Returns the context and initial serial number.
async fn setup_ei_context() -> Result<(ei::Context, u32), InputError> {
    use ashpd::desktop::remote_desktop::{DeviceType, RemoteDesktop};
    use ashpd::desktop::PersistMode;

    // Try LIBEI_SOCKET env var first (direct socket, no portal needed).
    if let Ok(Some(context)) = ei::Context::connect_to_env() {
        tracing::info!("Connected to ei via LIBEI_SOCKET");
        let resp = tokio::task::spawn_blocking(move || {
            ei_handshake_blocking(
                &context,
                "cosmic-rdp-server",
                ei::handshake::ContextType::Sender,
            )
            .map(|resp| (context, resp))
        })
        .await
        .map_err(|e| InputError::Init(format!("handshake task panicked: {e}")))?
        .map_err(|e| InputError::Init(format!("handshake failed: {e}")))?;

        return Ok((resp.0, resp.1.serial));
    }

    // Fall back to the RemoteDesktop portal.
    tracing::info!("No LIBEI_SOCKET, using RemoteDesktop portal");

    let remote_desktop = RemoteDesktop::new()
        .await
        .map_err(|e| InputError::Init(format!("RemoteDesktop proxy: {e}")))?;

    let session = remote_desktop
        .create_session()
        .await
        .map_err(|e| InputError::Init(format!("create session: {e}")))?;

    remote_desktop
        .select_devices(
            &session,
            DeviceType::Keyboard | DeviceType::Pointer,
            None,
            PersistMode::DoNot,
        )
        .await
        .map_err(|e| InputError::Init(format!("select devices: {e}")))?;

    let _response = remote_desktop
        .start(&session, None)
        .await
        .map_err(|e| InputError::Init(format!("start session: {e}")))?
        .response()
        .map_err(|e| InputError::Init(format!("start response: {e}")))?;

    let fd = remote_desktop
        .connect_to_eis(&session)
        .await
        .map_err(|e| InputError::Init(format!("connect to EIS: {e}")))?;

    let stream = UnixStream::from(fd);
    let context = ei::Context::new(stream)
        .map_err(|e| InputError::Init(format!("ei context: {e}")))?;

    tracing::info!("Connected to ei via RemoteDesktop portal");

    let resp = tokio::task::spawn_blocking(move || {
        ei_handshake_blocking(
            &context,
            "cosmic-rdp-server",
            ei::handshake::ContextType::Sender,
        )
        .map(|resp| (context, resp))
    })
    .await
    .map_err(|e| InputError::Init(format!("handshake task panicked: {e}")))?
    .map_err(|e| InputError::Init(format!("handshake failed: {e}")))?;

    Ok((resp.0, resp.1.serial))
}

/// Process initial events after handshake to discover seats,
/// capabilities, and devices. Returns a fully configured `EiInput`.
#[allow(clippy::too_many_lines)]
fn discover_devices(context: ei::Context, mut serial: u32) -> Result<EiInput, InputError> {
    // Track seats and their capabilities. We use the Seat object directly
    // as the key since the inner id field is private.
    let mut seats: HashMap<ei::Seat, HashMap<String, u64>> = HashMap::new();
    let mut device_data: Option<DeviceData> = None;
    let mut found_device: Option<ei::Device> = None;
    let mut resumed = false;

    // Process events in a tight loop with a short timeout.
    // The EIS server sends the seat/device info immediately after handshake.
    // Note: the handshake may have buffered post-handshake data (seat info)
    // in the context, so we must always drain pending events even on timeout.
    for iteration in 0..200 {
        // Poll for readable data with a short timeout
        let poll_result = rustix::event::poll(
            &mut [rustix::event::PollFd::new(
                &context,
                rustix::event::PollFlags::IN,
            )],
            50, // 50ms timeout
        );

        match poll_result {
            Ok(0) => {
                // Timeout - no new socket data, but still drain any
                // events already buffered from a previous read.
            }
            Ok(_) => {
                // New data available on socket - read it into the buffer
                context
                    .read()
                    .map_err(|e| InputError::Init(format!("read error: {e}")))?;
            }
            Err(e) => {
                return Err(InputError::Init(format!("poll error: {e}")));
            }
        }

        let mut got_events = false;
        while let Some(result) = context.pending_event() {
            got_events = true;
            let event = match result {
                PendingRequestResult::Request(event) => event,
                PendingRequestResult::ParseError(e) => {
                    tracing::warn!("Parse error during device discovery: {e:?}");
                    continue;
                }
                PendingRequestResult::InvalidObject(id) => {
                    tracing::warn!(id, "Invalid object during device discovery");
                    continue;
                }
            };

            match event {
                ei::Event::Connection(_connection, conn_event) => match conn_event {
                    ei::connection::Event::Seat { seat } => {
                        seats.insert(seat, HashMap::new());
                    }
                    ei::connection::Event::Ping { ping } => {
                        ping.done(0);
                    }
                    ei::connection::Event::Disconnected { .. } => {
                        return Err(InputError::Init(
                            "disconnected during discovery".to_string(),
                        ));
                    }
                    _ => {}
                },
                ei::Event::Seat(seat, seat_event) => match seat_event {
                    ei::seat::Event::Capability { mask, interface } => {
                        if let Some(caps) = seats.get_mut(&seat) {
                            caps.insert(interface, mask);
                        }
                    }
                    ei::seat::Event::Done => {
                        // Bind all capabilities the seat offers
                        if let Some(caps) = seats.get(&seat) {
                            let mut bind_mask: u64 = 0;
                            for mask in caps.values() {
                                bind_mask |= mask;
                            }
                            seat.bind(bind_mask);
                            let _ = context.flush();
                        }
                    }
                    ei::seat::Event::Device { device } => {
                        if found_device.is_none() {
                            found_device = Some(device);
                            device_data = Some(DeviceData::default());
                        }
                    }
                    _ => {}
                },
                ei::Event::Device(device, dev_event) => {
                    if found_device.as_ref().is_some_and(|d| *d == device) {
                        match dev_event {
                            ei::device::Event::Interface { object } => {
                                if let Some(ref mut data) = device_data {
                                    data.interfaces
                                        .insert(object.interface().to_string(), object);
                                }
                            }
                            ei::device::Event::Resumed { serial: s } => {
                                serial = s;
                                resumed = true;
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        let _ = context.flush();

        if found_device.is_some() && resumed {
            tracing::debug!(iteration, "Device discovery complete");
            break;
        }

        // Log progress periodically to aid debugging
        if got_events && iteration % 10 == 0 {
            tracing::debug!(
                iteration,
                seats = seats.len(),
                has_device = found_device.is_some(),
                resumed,
                "Device discovery progress"
            );
        }
    }

    let device = found_device
        .ok_or_else(|| InputError::Init("no input device found".to_string()))?;

    let data = device_data.unwrap_or_default();

    let keyboard = data.interface::<ei::Keyboard>();
    let pointer = data.interface::<ei::Pointer>();
    let pointer_abs = data.interface::<ei::PointerAbsolute>();
    let button = data.interface::<ei::Button>();
    let scroll = data.interface::<ei::Scroll>();

    tracing::info!(
        keyboard = keyboard.is_some(),
        pointer = pointer.is_some(),
        pointer_abs = pointer_abs.is_some(),
        button = button.is_some(),
        scroll = scroll.is_some(),
        "ei device capabilities"
    );

    Ok(EiInput {
        context,
        device,
        keyboard,
        pointer,
        pointer_abs,
        button,
        scroll,
        serial,
        sequence: 0,
        emulating: false,
        lock_state: LockState::default(),
    })
}

/// Errors from the input injection backend.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// Failed to initialize the reis/libei backend.
    #[error("failed to initialize input backend: {0}")]
    Init(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_state_default_all_off() {
        let state = LockState::default();
        assert!(!state.caps_lock);
        assert!(!state.num_lock);
        assert!(!state.scroll_lock);
    }

    #[test]
    fn lock_state_toggle_on_press() {
        let mut state = LockState::default();

        // First press toggles ON
        state.toggle_on_press(KEY_CAPSLOCK);
        assert!(state.caps_lock);
        assert!(!state.num_lock);

        // Second press toggles OFF
        state.toggle_on_press(KEY_CAPSLOCK);
        assert!(!state.caps_lock);

        // Num Lock toggle
        state.toggle_on_press(KEY_NUMLOCK);
        assert!(state.num_lock);

        // Scroll Lock toggle
        state.toggle_on_press(KEY_SCROLLLOCK);
        assert!(state.scroll_lock);

        // Non-lock key does nothing
        state.toggle_on_press(38); // 'A' key
        assert!(state.num_lock);
        assert!(state.scroll_lock);
    }

    #[test]
    fn locks_to_toggle_no_diff() {
        let current = LockState {
            caps_lock: true,
            num_lock: false,
            scroll_lock: true,
        };
        let target = current.clone();
        assert!(current.locks_to_toggle(&target).is_empty());
    }

    #[test]
    fn locks_to_toggle_all_different() {
        let current = LockState::default();
        let target = LockState {
            caps_lock: true,
            num_lock: true,
            scroll_lock: true,
        };
        let mut keys = current.locks_to_toggle(&target);
        keys.sort_unstable();
        assert_eq!(keys, vec![KEY_CAPSLOCK, KEY_NUMLOCK, KEY_SCROLLLOCK]);
    }

    #[test]
    fn locks_to_toggle_partial_diff() {
        let current = LockState {
            caps_lock: true,
            num_lock: false,
            scroll_lock: false,
        };
        let target = LockState {
            caps_lock: true,
            num_lock: true,
            scroll_lock: false,
        };
        let keys = current.locks_to_toggle(&target);
        assert_eq!(keys, vec![KEY_NUMLOCK]);
    }
}
