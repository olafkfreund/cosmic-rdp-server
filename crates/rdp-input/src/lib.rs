//! Input injection abstraction for cosmic-rdp-server.
//!
//! Provides keyboard and mouse injection into the COSMIC compositor
//! via `libei` (using the `enigo` crate).
//!
//! - [`keymap`]: RDP XT scancode to evdev keycode mapping
//! - [`libei`]: enigo/libei backend for input injection

pub mod keymap;
pub mod libei;

pub use keymap::rdp_scancode_to_evdev;
pub use libei::{EnigoInput, InputError};
