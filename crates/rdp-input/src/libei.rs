//! enigo/libei backend for input injection.
//!
//! Uses the `enigo` crate with the `libei_tokio` feature to inject
//! keyboard and mouse events into the COSMIC compositor via the
//! `libei` protocol.

use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};

use crate::keymap::rdp_scancode_to_evdev;

/// Input injector backed by enigo/libei.
///
/// Wraps an [`enigo::Enigo`] instance and provides methods matching
/// the RDP event model (scancodes, absolute mouse coordinates, scroll).
pub struct EnigoInput {
    enigo: Enigo,
}

impl EnigoInput {
    /// Create a new input injector.
    ///
    /// Connects to the compositor via libei. This may fail if the
    /// compositor does not support the ei protocol or the user has
    /// not granted permission.
    ///
    /// # Errors
    ///
    /// Returns [`InputError::Init`] if enigo cannot connect to the compositor.
    pub fn new() -> Result<Self, InputError> {
        let enigo = Enigo::new(&Settings::default()).map_err(|e| {
            InputError::Init(format!("{e}"))
        })?;
        Ok(Self { enigo })
    }

    /// Inject a keyboard key press.
    ///
    /// Converts the RDP XT scancode to an evdev keycode and sends a press event.
    pub fn key_press(&mut self, code: u8, extended: bool) {
        let Some(evdev) = rdp_scancode_to_evdev(code, extended) else {
            tracing::debug!(code, extended, "Unmapped scancode (press)");
            return;
        };
        if let Err(e) = self.enigo.raw(evdev, Direction::Press) {
            tracing::warn!(evdev, "Key press failed: {e}");
        }
    }

    /// Inject a keyboard key release.
    ///
    /// Converts the RDP XT scancode to an evdev keycode and sends a release event.
    pub fn key_release(&mut self, code: u8, extended: bool) {
        let Some(evdev) = rdp_scancode_to_evdev(code, extended) else {
            tracing::debug!(code, extended, "Unmapped scancode (release)");
            return;
        };
        if let Err(e) = self.enigo.raw(evdev, Direction::Release) {
            tracing::warn!(evdev, "Key release failed: {e}");
        }
    }

    /// Move the mouse to absolute coordinates.
    ///
    /// Coordinates are in desktop pixels as reported by the RDP client.
    pub fn mouse_move(&mut self, x: u16, y: u16) {
        if let Err(e) = self
            .enigo
            .move_mouse(i32::from(x), i32::from(y), Coordinate::Abs)
        {
            tracing::warn!(x, y, "Mouse move failed: {e}");
        }
    }

    /// Move the mouse by a relative offset.
    pub fn mouse_rel_move(&mut self, x: i32, y: i32) {
        if let Err(e) = self.enigo.move_mouse(x, y, Coordinate::Rel) {
            tracing::warn!(x, y, "Relative mouse move failed: {e}");
        }
    }

    /// Press or release a mouse button.
    pub fn mouse_button(&mut self, button: Button, direction: Direction) {
        if let Err(e) = self.enigo.button(button, direction) {
            tracing::warn!(?button, ?direction, "Mouse button failed: {e}");
        }
    }

    /// Scroll vertically.
    ///
    /// Positive values scroll down, negative scroll up (matching RDP convention).
    pub fn scroll_vertical(&mut self, value: i32) {
        if let Err(e) = self.enigo.scroll(value, Axis::Vertical) {
            tracing::warn!(value, "Vertical scroll failed: {e}");
        }
    }

    /// Scroll with explicit x/y amounts.
    pub fn scroll(&mut self, x: i32, y: i32) {
        if x != 0 {
            if let Err(e) = self.enigo.scroll(x, Axis::Horizontal) {
                tracing::warn!(x, "Horizontal scroll failed: {e}");
            }
        }
        if y != 0 {
            if let Err(e) = self.enigo.scroll(y, Axis::Vertical) {
                tracing::warn!(y, "Vertical scroll failed: {e}");
            }
        }
    }
}

/// Errors from the input injection backend.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// Failed to initialize the enigo/libei backend.
    #[error("failed to initialize input backend: {0}")]
    Init(String),
}
