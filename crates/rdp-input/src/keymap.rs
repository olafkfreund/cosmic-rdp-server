//! RDP XT scancode to evdev keycode mapping.
//!
//! RDP sends XT Set 1 scancodes (8-bit) with an `extended` flag for keys
//! that use the 0xE0 prefix in the PS/2 protocol. This module converts
//! them to Linux evdev keycodes for injection via libei/enigo.

/// Convert an RDP XT scancode to a Linux evdev keycode.
///
/// # Arguments
/// * `code` - The XT Set 1 scancode (0x00-0x7F range)
/// * `extended` - Whether the key has the extended (E0) prefix
///
/// # Returns
/// The evdev keycode, or `None` if the scancode is unmapped.
#[must_use]
pub fn rdp_scancode_to_evdev(code: u8, extended: bool) -> Option<u16> {
    if extended {
        extended_scancode_to_evdev(code)
    } else {
        standard_scancode_to_evdev(code)
    }
}

/// Map standard (non-extended) XT scancodes to evdev keycodes.
///
/// For most standard keys, evdev keycode = XT scancode + 8.
fn standard_scancode_to_evdev(code: u8) -> Option<u16> {
    #[allow(clippy::match_same_arms)]
    let evdev = match code {
        0x01 => 9,   // Escape
        0x02 => 10,  // 1
        0x03 => 11,  // 2
        0x04 => 12,  // 3
        0x05 => 13,  // 4
        0x06 => 14,  // 5
        0x07 => 15,  // 6
        0x08 => 16,  // 7
        0x09 => 17,  // 8
        0x0A => 18,  // 9
        0x0B => 19,  // 0
        0x0C => 20,  // Minus
        0x0D => 21,  // Equal
        0x0E => 22,  // Backspace
        0x0F => 23,  // Tab
        0x10 => 24,  // Q
        0x11 => 25,  // W
        0x12 => 26,  // E
        0x13 => 27,  // R
        0x14 => 28,  // T
        0x15 => 29,  // Y
        0x16 => 30,  // U
        0x17 => 31,  // I
        0x18 => 32,  // O
        0x19 => 33,  // P
        0x1A => 34,  // Left Bracket
        0x1B => 35,  // Right Bracket
        0x1C => 36,  // Enter
        0x1D => 37,  // Left Ctrl
        0x1E => 38,  // A
        0x1F => 39,  // S
        0x20 => 40,  // D
        0x21 => 41,  // F
        0x22 => 42,  // G
        0x23 => 43,  // H
        0x24 => 44,  // J
        0x25 => 45,  // K
        0x26 => 46,  // L
        0x27 => 47,  // Semicolon
        0x28 => 48,  // Apostrophe
        0x29 => 49,  // Grave (backtick)
        0x2A => 50,  // Left Shift
        0x2B => 51,  // Backslash
        0x2C => 52,  // Z
        0x2D => 53,  // X
        0x2E => 54,  // C
        0x2F => 55,  // V
        0x30 => 56,  // B
        0x31 => 57,  // N
        0x32 => 58,  // M
        0x33 => 59,  // Comma
        0x34 => 60,  // Period
        0x35 => 61,  // Slash
        0x36 => 62,  // Right Shift
        0x37 => 63,  // Keypad Asterisk
        0x38 => 64,  // Left Alt
        0x39 => 65,  // Space
        0x3A => 66,  // Caps Lock
        0x3B => 67,  // F1
        0x3C => 68,  // F2
        0x3D => 69,  // F3
        0x3E => 70,  // F4
        0x3F => 71,  // F5
        0x40 => 72,  // F6
        0x41 => 73,  // F7
        0x42 => 74,  // F8
        0x43 => 75,  // F9
        0x44 => 76,  // F10
        0x45 => 77,  // Num Lock
        0x46 => 78,  // Scroll Lock
        0x47 => 79,  // Keypad 7
        0x48 => 80,  // Keypad 8
        0x49 => 81,  // Keypad 9
        0x4A => 82,  // Keypad Minus
        0x4B => 83,  // Keypad 4
        0x4C => 84,  // Keypad 5
        0x4D => 85,  // Keypad 6
        0x4E => 86,  // Keypad Plus
        0x4F => 87,  // Keypad 1
        0x50 => 88,  // Keypad 2
        0x51 => 89,  // Keypad 3
        0x52 => 90,  // Keypad 0
        0x53 => 91,  // Keypad Period
        0x56 => 94,  // Intl Backslash (102nd key)
        0x57 => 95,  // F11
        0x58 => 96,  // F12
        _ => return None,
    };
    Some(evdev)
}

/// Map extended (E0-prefixed) XT scancodes to evdev keycodes.
///
/// Extended keys include navigation cluster, arrow keys, right-side
/// modifiers, and Windows/Menu keys.
fn extended_scancode_to_evdev(code: u8) -> Option<u16> {
    let evdev = match code {
        0x1C => 104, // Keypad Enter
        0x1D => 105, // Right Ctrl
        0x35 => 106, // Keypad Slash
        0x37 => 107, // Print Screen / SysRq
        0x38 => 108, // Right Alt
        0x46 => 127, // Pause / Break
        0x47 => 110, // Home
        0x48 => 111, // Up Arrow
        0x49 => 112, // Page Up
        0x4B => 113, // Left Arrow
        0x4D => 114, // Right Arrow
        0x4F => 115, // End
        0x50 => 116, // Down Arrow
        0x51 => 117, // Page Down
        0x52 => 118, // Insert
        0x53 => 119, // Delete
        0x5B => 133, // Left Super / Windows
        0x5C => 134, // Right Super / Windows
        0x5D => 135, // Menu / Compose
        _ => return None,
    };
    Some(evdev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_keys() {
        // Letter A: XT 0x1E -> evdev 38
        assert_eq!(rdp_scancode_to_evdev(0x1E, false), Some(38));
        // Space: XT 0x39 -> evdev 65
        assert_eq!(rdp_scancode_to_evdev(0x39, false), Some(65));
        // Enter: XT 0x1C -> evdev 36
        assert_eq!(rdp_scancode_to_evdev(0x1C, false), Some(36));
        // Escape: XT 0x01 -> evdev 9
        assert_eq!(rdp_scancode_to_evdev(0x01, false), Some(9));
        // F1: XT 0x3B -> evdev 67
        assert_eq!(rdp_scancode_to_evdev(0x3B, false), Some(67));
    }

    #[test]
    fn test_extended_keys() {
        // Right Ctrl: extended 0x1D -> evdev 105
        assert_eq!(rdp_scancode_to_evdev(0x1D, true), Some(105));
        // Up Arrow: extended 0x48 -> evdev 111
        assert_eq!(rdp_scancode_to_evdev(0x48, true), Some(111));
        // Left Super: extended 0x5B -> evdev 133
        assert_eq!(rdp_scancode_to_evdev(0x5B, true), Some(133));
        // Delete: extended 0x53 -> evdev 119
        assert_eq!(rdp_scancode_to_evdev(0x53, true), Some(119));
    }

    #[test]
    fn test_unmapped_returns_none() {
        assert_eq!(rdp_scancode_to_evdev(0x00, false), None);
        assert_eq!(rdp_scancode_to_evdev(0x7F, false), None);
        assert_eq!(rdp_scancode_to_evdev(0xFF, true), None);
    }
}
