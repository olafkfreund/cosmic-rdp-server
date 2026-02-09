# CLAUDE.md - cosmic-rdp-server

## Project Overview

RDP server for the COSMIC Desktop Environment. Allows remote desktop access using standard RDP clients (Windows mstsc.exe, FreeRDP, Remmina).

## Architecture

Standalone daemon (direct Wayland client) with 4 crates:

| Crate | Purpose |
|-------|---------|
| `cosmic-rdp-server` | Main binary: CLI, config, TLS, server orchestration |
| `rdp-capture` | Screen capture via ScreenCast portal + PipeWire |
| `rdp-input` | Input injection via enigo/libei |
| `rdp-encode` | Video encoding via GStreamer (H.264) + bitmap fallback |

## Build Commands

```bash
just                    # Build release (default)
just build-debug        # Debug build
just build-release      # Release build
just check              # Clippy with pedantic warnings
just run                # Run with RUST_BACKTRACE=full
just test               # Run tests
just clean              # Clean build artifacts
sudo just install       # Install to system
nix develop             # Enter dev shell with all dependencies
nix build               # Build with Nix
```

## Key Dependencies

- **ironrdp-server** ~0.10 (with `helper` feature): RDP protocol server
- **ashpd** ~0.12: XDG Desktop Portal (ScreenCast)
- **pipewire** ~0.8: PipeWire stream handling
- **gstreamer** ~0.23: H.264 encoding
- **enigo** 0.6 (libei_tokio): Input injection via libei
- **tokio-rustls** + **rcgen**: TLS + self-signed certificates

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
- **Phase 2**: Live screen capture (ScreenCast portal + PipeWire)
- **Phase 3**: Input injection (keyboard + mouse via libei)
- **Phase 4**: H.264 encoding (GStreamer EGFX pipeline)
- **Phase 5**: Config, auth, NixOS module
- **Phase 6**: Clipboard, multi-monitor, audio, polish
