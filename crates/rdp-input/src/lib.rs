// Input injection abstraction for cosmic-rdp-server.
//
// Provides the InputSink trait and implementations:
// - libei.rs: enigo/libei backend for COSMIC compositor
// - keymap.rs: RDP scancode to XKB keycode mapping

pub mod keymap;
pub mod libei;
