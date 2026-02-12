# COSMIC RDP Server

RDP server for the COSMIC Desktop Environment, providing remote access via standard RDP clients.

## Project Overview

- **Purpose:** Remote desktop access (capture, input, audio, clipboard) for COSMIC sessions.
- **Main Technologies:**
    - **Language:** Rust (edition 2021, 1.85+)
    - **Protocol:** RDP (via `ironrdp`)
    - **Capture:** XDG ScreenCast Portal + PipeWire
    - **Input:** reis/libei (direct libei protocol)
    - **Audio:** RDPSND via PipeWire
    - **Video Encoding:** H.264 (via GStreamer) with EGFX support, bitmap fallback.
    - **GUI:** `libcosmic` (iced-based) for settings management.
    - **IPC:** D-Bus (via `zbus`) for daemon/GUI communication.
    - **Build/Env:** Nix (flakes), `just` (command runner).

## Architecture

The project is structured as a Rust workspace with 6 crates:

- `cosmic-ext-rdp-server`: Main daemon (CLI, config, orchestration).
- `cosmic-ext-rdp-settings`: COSMIC Settings GUI.
- `rdp-dbus`: Shared D-Bus types and IPC logic.
- `rdp-capture`: Portal and PipeWire screen capture.
- `rdp-input`: libei input injection.
- `rdp-encode`: GStreamer video encoding.

## Building and Running

### Development Environment
Use Nix to enter a shell with all dependencies:
```bash
nix develop
```

### Key Commands (via `just`)
- `just build-release`: Build the server in release mode.
- `just build-settings-release`: Build the settings GUI in release mode.
- `just run`: Run the server with `RUST_BACKTRACE=full`.
- `just run-settings`: Run the settings GUI.
- `just test`: Run all workspace tests.
- `just check`: Run Clippy with pedantic warnings.
- `just fmt`: Format code.

### Manual Execution
- `cosmic-ext-rdp-server --addr 0.0.0.0 --port 3389`: Start server.
- `cosmic-ext-rdp-server --static-display`: Start with a blue screen (no portal needed).

## Development Conventions

- **Async:** Uses `tokio` for the async runtime.
- **Logging:** Uses `tracing` and `tracing-subscriber`. Configure with `RUST_LOG`.
- **Error Handling:** Uses `anyhow` for applications and `thiserror` for library crates.
- **Formatting:** Standard `rustfmt` via `just fmt`.
- **Linting:** Clippy is used with `pedantic` warnings in the CI/check recipes.
- **Nix:** NixOS and Home Manager modules are provided in the `nix/` directory.
- **Portals:** Interacts with `xdg-desktop-portal-cosmic` for ScreenCast and RemoteDesktop.

## Important Notes

- **NLA Authentication:** Required when binding to non-localhost addresses unless explicitly disabled in dev.
- **TLS:** Generates self-signed certificates by default; can be configured to use custom PEM files.
- **Full Stack:** Requires a compatible portal (`xdg-desktop-portal-cosmic`) and compositor (`cosmic-comp-rdp`) for input injection to work.
