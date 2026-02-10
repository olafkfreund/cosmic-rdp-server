# cosmic-rdp-server

RDP server for the [COSMIC Desktop Environment](https://github.com/pop-os/cosmic-epoch). Allows remote desktop access to COSMIC sessions using standard RDP clients such as Windows Remote Desktop (`mstsc.exe`), FreeRDP, and Remmina.

Part of the [COSMIC Remote Desktop stack](#full-remote-desktop-stack) - works together with [xdg-desktop-portal-cosmic](https://github.com/olafkfreund/xdg-desktop-portal-cosmic) (portal) and [cosmic-comp-rdp](https://github.com/olafkfreund/cosmic-comp-rdp) (compositor) for full remote desktop functionality.

## Features

- **Live screen capture** via the ScreenCast XDG portal and PipeWire
- **Keyboard and mouse injection** via reis/libei (direct libei protocol)
- **Clipboard sharing** (text) between local and remote sessions via CLIPRDR
- **Audio forwarding** from the desktop to the RDP client via RDPSND + PipeWire
- **Dynamic display resize** when the client window changes size
- **Cursor shape forwarding** (position, RGBA bitmap, hide/show)
- **Lock key synchronization** (Caps Lock, Num Lock, Scroll Lock state sync)
- **NLA authentication** via CredSSP (optional)
- **TLS encryption** with self-signed certificates or user-provided PEM files
- **H.264 encoding** pipeline ready (GStreamer with VAAPI/NVENC/software fallback; awaiting upstream EGFX support in ironrdp-server)
- **COSMIC Settings GUI** for configuration management via D-Bus IPC
- **NixOS module** with systemd service, firewall integration, and secrets management
- **Home Manager module** for user-level installation
- **Graceful shutdown** on SIGINT/SIGTERM and D-Bus stop/reload commands
- **View-only fallback** when input injection is unavailable

## Architecture

### Crate overview

Workspace with 6 crates:

| Crate | Purpose |
|-------|---------|
| `cosmic-rdp-server` | Main daemon: CLI, config, TLS, D-Bus server, orchestration |
| `cosmic-rdp-settings` | COSMIC Settings GUI: config editor, D-Bus status, nav pages |
| `rdp-dbus` | Shared D-Bus types, config structs, client/server proxy |
| `rdp-capture` | Screen capture via ScreenCast portal + PipeWire |
| `rdp-input` | Input injection via reis/libei (direct libei protocol) |
| `rdp-encode` | Video encoding via GStreamer (H.264) + bitmap fallback |

### Data flow

```
RDP Client
    |
    v
cosmic-rdp-server (this repo)
    |
    |-- ScreenCast portal --> PipeWire --> rdp-capture --> rdp-encode --> RDP bitmap/video
    |-- RemoteDesktop portal --> EIS socket --> rdp-input --> compositor keyboard/mouse
    |-- CLIPRDR channel <--> arboard --> system clipboard
    |-- RDPSND channel <-- PipeWire audio monitor
    |-- D-Bus IPC <--> cosmic-rdp-settings (GUI)
    v
ironrdp-server (RDP protocol)
```

### D-Bus interfaces

| Interface | Bus | Purpose |
|-----------|-----|---------|
| `com.system76.CosmicRdpServer` | Session | Daemon status, reload, stop (settings GUI IPC) |
| `org.freedesktop.impl.portal.RemoteDesktop` | Session | Portal for input injection (called by rdp-input) |
| `org.freedesktop.impl.portal.ScreenCast` | Session | Portal for screen capture (called by rdp-capture) |

## Requirements

- **COSMIC Desktop** (Wayland compositor with XDG portals)
- **PipeWire** (screen capture and audio)
- **libei** (input injection via the libei protocol)
- **GStreamer 1.x** with plugins-base, plugins-good, plugins-bad (video encoding)
- **Rust 1.85+** (edition 2021)

### Full stack requirements

For full remote desktop functionality (capture + input), you also need:
- [xdg-desktop-portal-cosmic](https://github.com/olafkfreund/xdg-desktop-portal-cosmic) with RemoteDesktop support
- [cosmic-comp-rdp](https://github.com/olafkfreund/cosmic-comp-rdp) with EIS receiver

See the [Full Remote Desktop Stack](#full-remote-desktop-stack) section for setup instructions.

## Building

### Using Nix (recommended)

```bash
nix develop              # Enter dev shell with all dependencies
just build-release       # Build release binary
just test                # Run tests

# Or build directly with Nix
nix build                           # Build server
nix build .#cosmic-rdp-settings     # Build settings GUI
```

### Using Cargo (requires system libraries)

Install the required development headers for your distribution:

**Fedora/RHEL:**
```bash
sudo dnf install pipewire-devel libei-devel wayland-devel libxkbcommon-devel \
  gstreamer1-devel gstreamer1-plugins-base-devel openssl-devel \
  fontconfig-devel freetype-devel mesa-libGL-devel mesa-libEGL-devel \
  vulkan-loader-devel dbus-devel clang-devel
```

**Debian/Ubuntu:**
```bash
sudo apt install libpipewire-0.3-dev libei-dev libwayland-dev libxkbcommon-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libssl-dev \
  libfontconfig-dev libfreetype-dev libgl-dev libegl-dev \
  libvulkan-dev libdbus-1-dev clang
```

**Arch Linux:**
```bash
sudo pacman -S pipewire libei wayland libxkbcommon gstreamer gst-plugins-base \
  openssl fontconfig freetype2 mesa vulkan-icd-loader dbus clang
```

Then build:
```bash
cargo build --release
```

### Build commands (justfile)

```bash
just                           # Build release (default)
just build-debug               # Debug build
just build-release             # Release build
just build-settings-debug      # Build settings GUI (debug)
just build-settings-release    # Build settings GUI (release)
just check                     # Clippy with pedantic warnings
just run                       # Run server with RUST_BACKTRACE=full
just run-settings              # Run settings GUI
just test                      # Run all workspace tests
just fmt                       # Format code
just clean                     # Clean build artifacts
sudo just install              # Install server to /usr/bin + desktop entry
sudo just install-settings     # Install settings GUI to /usr/bin + desktop entry
sudo just install-all          # Install everything
```

### Building an AUR package (Arch Linux)

Create a `PKGBUILD`:

```bash
# Maintainer: Your Name <you@example.com>
pkgname=cosmic-rdp-server
pkgver=0.1.0
pkgrel=1
pkgdesc="RDP server for the COSMIC Desktop Environment"
arch=('x86_64' 'aarch64')
url="https://github.com/olafkfreund/cosmic-rdp-server"
license=('GPL-3.0-only')
depends=('pipewire' 'libei' 'wayland' 'libxkbcommon' 'gstreamer' 'gst-plugins-base'
         'gst-plugins-good' 'gst-plugins-bad' 'gst-plugin-va' 'openssl' 'dbus')
makedepends=('cargo' 'just' 'clang' 'pkg-config')
source=("$pkgname-$pkgver.tar.gz::$url/archive/v$pkgver.tar.gz")
sha256sums=('SKIP')

prepare() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  just build-release
  just build-settings-release
}

package() {
  cd "$pkgname-$pkgver"
  just rootdir="$pkgdir" install-all
}
```

Build and install:
```bash
makepkg -si
```

### Building a Debian package

Create the `debian/` directory structure:

```bash
mkdir -p debian/source
```

**`debian/control`:**
```
Source: cosmic-rdp-server
Section: net
Priority: optional
Maintainer: Your Name <you@example.com>
Build-Depends: debhelper-compat (= 13), cargo, rustc (>= 1.85),
 just, clang, pkg-config, libpipewire-0.3-dev, libei-dev,
 libwayland-dev, libxkbcommon-dev, libgstreamer1.0-dev,
 libgstreamer-plugins-base1.0-dev, libssl-dev, libfontconfig-dev,
 libfreetype-dev, libgl-dev, libvulkan-dev, libdbus-1-dev
Standards-Version: 4.7.0
Homepage: https://github.com/olafkfreund/cosmic-rdp-server

Package: cosmic-rdp-server
Architecture: any
Depends: ${shlibs:Depends}, ${misc:Depends}, pipewire, libei1,
 gstreamer1.0-plugins-base, gstreamer1.0-plugins-good,
 gstreamer1.0-plugins-bad, gstreamer1.0-vaapi
Description: RDP server for the COSMIC Desktop Environment
 Allows remote desktop access to COSMIC sessions using standard
 RDP clients. Supports live screen capture, keyboard/mouse injection,
 clipboard sharing, audio forwarding, and dynamic display resizing.
```

**`debian/rules`:**
```makefile
#!/usr/bin/make -f
%:
	dh $@

override_dh_auto_build:
	just build-release
	just build-settings-release

override_dh_auto_install:
	just rootdir=debian/cosmic-rdp-server install-all
```

**`debian/changelog`:**
```
cosmic-rdp-server (0.1.0-1) unstable; urgency=medium

  * Initial release.

 -- Your Name <you@example.com>  Mon, 10 Feb 2026 00:00:00 +0000
```

**`debian/source/format`:**
```
3.0 (quilt)
```

Build the package:
```bash
dpkg-buildpackage -us -uc -b
# Or using debuild:
debuild -us -uc -b
```

## Usage

### Quick start

```bash
# Start the server with defaults (binds to 0.0.0.0:3389, self-signed TLS)
cosmic-rdp-server

# Specify a custom address and port
cosmic-rdp-server --addr 0.0.0.0 --port 13389

# Use a custom TLS certificate
cosmic-rdp-server --cert /path/to/cert.pem --key /path/to/key.pem

# Use a configuration file
cosmic-rdp-server --config /path/to/config.toml

# Start with a static blue screen (for testing, no portal needed)
cosmic-rdp-server --static-display
```

### CLI options

| Flag | Description |
|------|-------------|
| `--addr <ADDRESS>` | Bind address (default: `0.0.0.0`) |
| `--port <PORT>` | Listen port (default: `3389`) |
| `--cert <PATH>` | TLS certificate file (PEM format) |
| `--key <PATH>` | TLS private key file (PEM format) |
| `--config`, `-c <PATH>` | Configuration file (TOML) |
| `--static-display` | Use a static blue screen instead of live capture |

### Connecting from a client

```bash
# FreeRDP (Linux)
xfreerdp /v:hostname:3389 /cert:ignore

# FreeRDP with NLA authentication
xfreerdp /v:hostname:3389 /u:myuser /p:mypassword /cert:ignore

# FreeRDP with dynamic resize
xfreerdp /v:hostname:3389 /cert:ignore /dynamic-resolution

# Remmina (Linux GUI)
# Create a new RDP connection, set Server to hostname:3389

# Windows Remote Desktop (mstsc.exe)
mstsc /v:hostname:3389
```

## Configuration

Configuration is read from TOML. Default location: `$XDG_CONFIG_HOME/cosmic-rdp-server/config.toml` (`~/.config/cosmic-rdp-server/config.toml`).

### Full example

```toml
# Network
bind = "0.0.0.0:3389"

# TLS (omit for self-signed)
# cert_path = "/etc/cosmic-rdp-server/cert.pem"
# key_path = "/etc/cosmic-rdp-server/key.pem"

# Static blue screen mode (for testing)
static_display = false

# NLA Authentication (CredSSP)
[auth]
enable = false
username = ""
password = ""
# domain = "WORKGROUP"

# Screen capture
[capture]
fps = 30
channel_capacity = 4
multi_monitor = false

# Video encoding
[encode]
encoder = "auto"       # "auto", "vaapi", "nvenc", or "software"
preset = "ultrafast"
bitrate = 10000000     # bits per second

# Clipboard sharing
[clipboard]
enable = true

# Audio forwarding (RDPSND)
[audio]
enable = true
sample_rate = 44100
channels = 2
```

### Configuration sections

#### `[auth]` - NLA Authentication

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | bool | `false` | Enable NLA via CredSSP |
| `username` | string | `""` | Username for authentication |
| `password` | string | `""` | Password for authentication |
| `domain` | string | `null` | Windows domain (optional) |

#### `[capture]` - Screen Capture

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `fps` | int | `30` | Target frames per second |
| `channel_capacity` | int | `4` | PipeWire frame buffer depth |
| `multi_monitor` | bool | `false` | Merge all monitors into a single virtual desktop |

#### `[encode]` - Video Encoding

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `encoder` | string | `"auto"` | Encoder backend: `auto`, `vaapi`, `nvenc`, `software` |
| `preset` | string | `"ultrafast"` | H.264 encoding preset |
| `bitrate` | int | `10000000` | Target bitrate in bits/second |

#### `[clipboard]` - Clipboard Sharing

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | bool | `true` | Enable text clipboard sharing via CLIPRDR |

#### `[audio]` - Audio Forwarding

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enable` | bool | `true` | Enable RDPSND audio forwarding |
| `sample_rate` | int | `44100` | Sample rate in Hz |
| `channels` | int | `2` | Number of audio channels (1=mono, 2=stereo) |

## Installation

### NixOS Module

The flake provides a NixOS module for declarative configuration.

#### Basic setup

```nix
{
  inputs.cosmic-rdp-server.url = "github:olafkfreund/cosmic-rdp-server";

  outputs = { self, nixpkgs, cosmic-rdp-server, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        cosmic-rdp-server.nixosModules.default
        {
          nixpkgs.overlays = [ cosmic-rdp-server.overlays.default ];

          services.cosmic-rdp-server = {
            enable = true;
            openFirewall = true;

            settings = {
              bind = "0.0.0.0:3389";
              capture.fps = 30;
              audio.enable = true;
              clipboard.enable = true;
            };
          };
        }
      ];
    };
  };
}
```

#### With NLA authentication

```nix
services.cosmic-rdp-server = {
  enable = true;
  openFirewall = true;

  auth = {
    enable = true;
    username = "rdpuser";
    # Password is loaded via systemd LoadCredential (never in Nix store).
    # Compatible with agenix, sops-nix, or any file-based secrets manager.
    passwordFile = "/run/agenix/cosmic-rdp-password";
  };

  settings = {
    bind = "0.0.0.0:3389";
  };
};
```

#### NixOS module options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | `false` | Enable the COSMIC RDP Server service |
| `package` | package | `pkgs.cosmic-rdp-server` | Server package to use |
| `installSettings` | bool | `true` | Install the COSMIC Settings GUI |
| `settingsPackage` | package | `pkgs.cosmic-rdp-settings` | Settings GUI package |
| `openFirewall` | bool | `false` | Open the RDP port in the firewall |
| `auth.enable` | bool | `false` | Enable NLA authentication |
| `auth.username` | string | `""` | NLA username |
| `auth.domain` | string | `null` | NLA domain (optional) |
| `auth.passwordFile` | path | `null` | Path to password file (loaded via `LoadCredential`) |
| `settings` | attrs | `{}` | TOML configuration (see Configuration section) |

The systemd service runs as a user service (`graphical-session.target`) with security hardening (no new privileges, read-only home, private tmp, restricted syscalls).

### Home Manager Module

For user-level installation without system-wide NixOS changes.

#### Basic setup

```nix
{
  inputs.cosmic-rdp-server.url = "github:olafkfreund/cosmic-rdp-server";

  outputs = { self, nixpkgs, home-manager, cosmic-rdp-server, ... }: {
    homeConfigurations."user" = home-manager.lib.homeManagerConfiguration {
      modules = [
        cosmic-rdp-server.homeManagerModules.default
        {
          nixpkgs.overlays = [ cosmic-rdp-server.overlays.default ];

          services.cosmic-rdp-server = {
            enable = true;
            autoStart = true;

            settings = {
              bind = "0.0.0.0:3389";
              capture.fps = 30;
              audio.enable = true;
            };
          };
        }
      ];
    };
  };
}
```

#### With NLA authentication (Home Manager)

```nix
services.cosmic-rdp-server = {
  enable = true;
  autoStart = true;

  auth = {
    enable = true;
    username = "rdpuser";
    passwordFile = "/run/agenix/cosmic-rdp-password";
  };

  settings.bind = "0.0.0.0:3389";
};
```

#### Home Manager options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | `false` | Enable the COSMIC RDP Server |
| `package` | package | `pkgs.cosmic-rdp-server` | Server package to use |
| `installSettings` | bool | `true` | Install the COSMIC Settings GUI |
| `settingsPackage` | package | `pkgs.cosmic-rdp-settings` | Settings GUI package |
| `autoStart` | bool | `false` | Start with the graphical session |
| `auth.enable` | bool | `false` | Enable NLA authentication |
| `auth.username` | string | `""` | NLA username |
| `auth.domain` | string | `null` | NLA domain (optional) |
| `auth.passwordFile` | path | `null` | Path to password file (loaded via `LoadCredential`) |
| `settings` | attrs | `{}` | TOML configuration (see Configuration section) |

The Home Manager service includes the same systemd security hardening as the NixOS module.

### Manual installation

After building with `just build-release` and `just build-settings-release`:

```bash
sudo just install          # Install server to /usr/bin + desktop entry
sudo just install-settings # Install settings GUI to /usr/bin + desktop entry
```

To uninstall:
```bash
sudo just uninstall-all
```

## Full Remote Desktop Stack

For a complete remote desktop setup on COSMIC, you need three components working together:

```
                                    +-----------------------+
                                    |  cosmic-comp-rdp      |
                                    |  (compositor + EIS)   |
                                    +-----------^-----------+
                                                |
                                    AcceptEisSocket(fd)
                                                |
+------------+     +-------------------+     +--+--------------------------+
| RDP Client | --> | cosmic-rdp-server | --> | xdg-desktop-portal-cosmic   |
| (mstsc,    |     | (this repo)       |     | (RemoteDesktop + ScreenCast)|
| FreeRDP,   | <-- | RDP protocol,     | <-- | EIS socket pairs,          |
| Remmina)   |     | TLS, auth         |     | PipeWire streams            |
+------------+     +-------------------+     +-----------------------------+
```

| Component | Repository | Purpose |
|-----------|-----------|---------|
| [cosmic-rdp-server](https://github.com/olafkfreund/cosmic-rdp-server) | This repo | RDP protocol server, capture + input orchestration |
| [xdg-desktop-portal-cosmic](https://github.com/olafkfreund/xdg-desktop-portal-cosmic) | Portal fork | RemoteDesktop + ScreenCast portal interfaces |
| [cosmic-comp-rdp](https://github.com/olafkfreund/cosmic-comp-rdp) | Compositor fork | EIS receiver for input injection |

### How the components interact

1. **cosmic-rdp-server** starts and calls the **RemoteDesktop** portal to request input injection and the **ScreenCast** portal to request screen capture
2. **xdg-desktop-portal-cosmic** shows a consent dialog, creates a UNIX socket pair for EIS, and sends the server-side fd to the compositor
3. **cosmic-comp-rdp** receives the EIS socket via D-Bus (`AcceptEisSocket`) and creates a seat with keyboard, pointer, and touch capabilities
4. The RDP server receives the client-side EIS socket via `ConnectToEIS` and sends input events through it
5. PipeWire streams carry screen frames from the compositor to the RDP server for encoding and delivery

### NixOS example (all three components)

```nix
{
  inputs = {
    cosmic-rdp-server.url = "github:olafkfreund/cosmic-rdp-server";
    xdg-desktop-portal-cosmic.url = "github:olafkfreund/xdg-desktop-portal-cosmic";
    cosmic-comp.url = "github:olafkfreund/cosmic-comp-rdp";
  };

  outputs = { self, nixpkgs, cosmic-rdp-server, xdg-desktop-portal-cosmic, cosmic-comp, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        cosmic-rdp-server.nixosModules.default
        xdg-desktop-portal-cosmic.nixosModules.default
        cosmic-comp.nixosModules.default
        {
          nixpkgs.overlays = [
            cosmic-rdp-server.overlays.default
            xdg-desktop-portal-cosmic.overlays.default
            cosmic-comp.overlays.default
          ];

          # Compositor with EIS support
          services.cosmic-comp.enable = true;

          # Portal with RemoteDesktop interface
          services.xdg-desktop-portal-cosmic.enable = true;

          # RDP server
          services.cosmic-rdp-server = {
            enable = true;
            openFirewall = true;
            settings.bind = "0.0.0.0:3389";
          };
        }
      ];
    };
  };
}
```

### Home Manager example (all three components)

```nix
{
  inputs = {
    cosmic-rdp-server.url = "github:olafkfreund/cosmic-rdp-server";
    xdg-desktop-portal-cosmic.url = "github:olafkfreund/xdg-desktop-portal-cosmic";
    cosmic-comp.url = "github:olafkfreund/cosmic-comp-rdp";
  };

  outputs = { self, nixpkgs, home-manager, cosmic-rdp-server, xdg-desktop-portal-cosmic, cosmic-comp, ... }: {
    homeConfigurations."user" = home-manager.lib.homeManagerConfiguration {
      modules = [
        cosmic-rdp-server.homeManagerModules.default
        xdg-desktop-portal-cosmic.homeManagerModules.default
        cosmic-comp.homeManagerModules.default
        {
          nixpkgs.overlays = [
            cosmic-rdp-server.overlays.default
            xdg-desktop-portal-cosmic.overlays.default
            cosmic-comp.overlays.default
          ];

          services.cosmic-rdp-server = {
            enable = true;
            autoStart = true;
            settings.bind = "0.0.0.0:3389";
          };

          services.xdg-desktop-portal-cosmic.enable = true;

          wayland.compositor.cosmic-comp.enable = true;
        }
      ];
    };
  };
}
```

### Component compatibility

All three repositories are tested together and use compatible dependency versions:

| Dependency | cosmic-rdp-server | xdg-desktop-portal-cosmic | cosmic-comp-rdp |
|------------|-------------------|---------------------------|-----------------|
| reis (libei) | 0.5 | 0.5 | 0.5 |
| zbus (D-Bus) | 5.x | 5.x | 5.x |
| ashpd (portals) | 0.12 | 0.12 | - |
| pipewire | 0.8 | git (freedesktop) | - |

D-Bus interface chain (verified compatible):
- Portal exposes `org.freedesktop.impl.portal.RemoteDesktop` with `ConnectToEIS`
- Portal calls `com.system76.CosmicComp.RemoteDesktop.AcceptEisSocket(fd)` on the compositor
- RDP server exposes `com.system76.CosmicRdpServer` for settings GUI IPC

## D-Bus Interface

The daemon exposes a D-Bus interface at `com.system76.CosmicRdpServer` on the session bus for IPC with the settings GUI:

- **Properties:** `Status` (Running/Stopped/Error), `BindAddress`
- **Methods:** `Reload`, `Stop`
- **Signals:** Status change notifications

The settings GUI (`cosmic-rdp-settings`) communicates with the daemon over this interface to display server status and trigger configuration reloads.

## Logging

The server uses `tracing` with `RUST_LOG` environment variable support:

```bash
# Default (info level)
cosmic-rdp-server

# Debug logging
RUST_LOG=debug cosmic-rdp-server

# Trace logging for specific crates
RUST_LOG=rdp_capture=trace,rdp_input=debug cosmic-rdp-server
```

## Troubleshooting

### No screen capture (black screen)

- Ensure PipeWire is running: `systemctl --user status pipewire`
- Ensure the ScreenCast portal is available: `busctl --user list | grep portal`
- Check that `xdg-desktop-portal-cosmic` is installed and running
- Try `--static-display` flag to verify the RDP connection itself works

### No input (keyboard/mouse not working)

- Ensure `xdg-desktop-portal-cosmic` with RemoteDesktop support is installed
- Ensure `cosmic-comp-rdp` with EIS receiver is running as the compositor
- Check the consent dialog was accepted (the portal shows a dialog on first connection)
- Check logs: `RUST_LOG=rdp_input=debug cosmic-rdp-server`

### Connection refused

- Check the server is running: `systemctl --user status cosmic-rdp-server`
- Check firewall rules: port 3389 (or custom port) must be open
- For NixOS: set `openFirewall = true` in the module configuration

### Audio not working

- Ensure PipeWire is running with audio support
- Check `[audio] enable = true` in the configuration
- Ensure the RDP client supports RDPSND (FreeRDP does by default)

## Known Limitations

- **Single client:** Only one RDP client can connect at a time
- **H.264 delivery:** The GStreamer H.264 encoder is ready but EGFX frame delivery is blocked on upstream support in ironrdp-server (bitmap fallback is used)
- **Cursor shapes:** SPA cursor metadata extraction requires unsafe FFI not yet implemented; cursor position is forwarded but custom cursor bitmaps from PipeWire are stubbed
- **Unicode input:** Unicode key events (IME) are not yet supported

## License

GPL-3.0-only
