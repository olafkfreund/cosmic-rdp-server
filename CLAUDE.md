# CLAUDE.md - cosmic-rdp-server

## Project Overview

RDP server for the COSMIC Desktop Environment. Allows remote desktop access using standard RDP clients (Windows mstsc.exe, FreeRDP, Remmina).

## Architecture

Standalone daemon (direct Wayland client) with 6 crates:

| Crate | Purpose |
|-------|---------|
| `cosmic-rdp-server` | Main binary: CLI, config, TLS, D-Bus server, orchestration |
| `cosmic-rdp-settings` | COSMIC settings GUI: config editor, D-Bus status, nav pages |
| `rdp-dbus` | Shared D-Bus types, config structs, client proxy |
| `rdp-capture` | Screen capture via ScreenCast portal + PipeWire |
| `rdp-input` | Input injection via enigo/libei |
| `rdp-encode` | Video encoding via GStreamer (H.264) + bitmap fallback |

## Build Commands

```bash
just                    # Build release (default)
just build-debug        # Debug build
just build-release      # Release build
just build-settings-debug   # Build settings GUI (debug)
just build-settings-release # Build settings GUI (release)
just check              # Clippy with pedantic warnings
just run                # Run server with RUST_BACKTRACE=full
just run-settings       # Run settings GUI
just test               # Run tests
just clean              # Clean build artifacts
sudo just install       # Install server to system
sudo just install-settings  # Install settings GUI to system
sudo just install-all   # Install everything
nix develop             # Enter dev shell with all dependencies
nix build               # Build server with Nix
nix build .#cosmic-rdp-settings  # Build settings GUI with Nix
```

## Key Dependencies

- **ironrdp-server** ~0.10 (with `helper` feature): RDP protocol server
- **ironrdp-cliprdr** ~0.5: CLIPRDR clipboard virtual channel
- **ironrdp-rdpsnd** ~0.7: RDPSND audio virtual channel
- **ashpd** ~0.12: XDG Desktop Portal (ScreenCast)
- **pipewire** ~0.8: PipeWire stream handling (video + audio capture)
- **gstreamer** ~0.23: H.264 encoding
- **enigo** 0.6 (libei_tokio): Input injection via libei
- **arboard** 3: System clipboard access (Wayland + X11)
- **tokio-rustls** + **rcgen**: TLS + self-signed certificates
- **zbus** 5: D-Bus IPC between daemon and settings GUI
- **libcosmic** (git): COSMIC Desktop application framework (settings GUI)

## Code Style

- Rust edition 2021, MSRV 1.85
- `clippy::pedantic` enforced via `just check`
- `thiserror` for library errors, `anyhow` in binary
- `tracing` for logging (never `println!`)
- No `unwrap()` in production code

## Reference Code

Patterns adapted from `cosmic-display-stream`:
- Screen capture: `capture.rs` (portal sessions, VideoFrame type)
- PipeWire: `pipewire.rs` (dedicated thread, SHM/DMA-BUF)
- Encoding: `encoder.rs` (GStreamer pipeline, hardware detection)
- Input: `input.rs` (enigo/libei lazy init, coordinate mapping)

## Implementation Phases

- **Phase 0**: Project scaffolding (DONE)
- **Phase 1**: Static blue screen MVP (DONE - ironrdp-server integration, TLS, static display)
- **Phase 2**: Live screen capture (DONE - ScreenCast portal + PipeWire)
- **Phase 3**: Input injection (DONE - keyboard + mouse via libei/enigo)
- **Phase 4**: H.264 encoding (DONE - GStreamer pipeline ready, EGFX blocked on upstream ironrdp)
- **Phase 5**: Config, auth, NixOS module (DONE - TOML config, NLA/CredSSP, NixOS module)
- **Phase 6**: Clipboard, dynamic resize, graceful shutdown (DONE - CLIPRDR text, display resize, SIGINT/SIGTERM)
- **Phase 7**: Audio forwarding, multi-monitor, cursor shape (DONE - RDPSND via PipeWire, compositor, cursor metadata)
- **Phase 8**: COSMIC Settings UI (DONE - settings GUI, D-Bus IPC, config editor, NixOS module update)
