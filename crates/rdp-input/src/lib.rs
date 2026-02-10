//! Input injection abstraction for cosmic-rdp-server.
//!
//! Provides keyboard and mouse injection into the COSMIC compositor
//! via `libei` (using the `reis` crate for direct protocol access).
//!
//! - [`keymap`]: RDP XT scancode to evdev keycode mapping
//! - [`libei`]: reis/libei backend for input injection

pub mod keymap;
pub mod libei;

pub use keymap::rdp_scancode_to_evdev;
pub use libei::{EiInput, InputError, LockState, MouseButton};
