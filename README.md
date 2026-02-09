# cosmic-rdp-server

RDP server for the COSMIC Desktop Environment. Allows remote desktop access using standard RDP clients (Windows mstsc.exe, FreeRDP, Remmina).

## Status

**Phase 0: Scaffolding** - Project structure created, dependencies resolved.

## Building

Requires NixOS or system libraries for GStreamer, PipeWire, libei, Wayland.

```bash
# Using Nix (recommended)
nix develop          # Enter dev shell with all dependencies
just build-release   # Build release binary

# Or with Nix directly
nix build            # Build package
```

## Architecture

| Crate | Purpose |
|-------|---------|
| `cosmic-rdp-server` | Main daemon: CLI, config, TLS, RDP server orchestration |
| `rdp-capture` | Screen capture via ScreenCast portal + PipeWire |
| `rdp-input` | Input injection via enigo/libei |
| `rdp-encode` | Video encoding via GStreamer H.264 + bitmap fallback |

## License

GPL-3.0-only