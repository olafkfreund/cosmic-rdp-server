# cosmic-ext-rdp-server

Multi-user RDP server for the [COSMIC Desktop Environment](https://github.com/pop-os/cosmic-epoch). Provides concurrent remote desktop access for multiple users with per-session isolation, PAM authentication, and automatic session lifecycle management. Supports standard RDP clients such as Windows Remote Desktop (`mstsc.exe`), FreeRDP, and Remmina.

Part of the [COSMIC Remote Desktop stack](#full-remote-desktop-stack) - works together with [xdg-desktop-portal-cosmic](https://github.com/olafkfreund/xdg-desktop-portal-cosmic) (portal) and [cosmic-comp-rdp](https://github.com/olafkfreund/cosmic-comp-rdp) (compositor) for full remote desktop functionality.

## Screenshots

### RDP Session (FreeRDP connected to COSMIC Desktop)

![RDP Session](docs/screenshots/rdp-session.png)

### COSMIC RDP Server Settings GUI

| General | Security |
|---------|----------|
| ![General Settings](docs/screenshots/settings-general.png) | ![Security Settings](docs/screenshots/settings-security.png) |

| Display | Features |
|---------|----------|
| ![Display Settings](docs/screenshots/settings-display.png) | ![Features Settings](docs/screenshots/settings-features.png) |

## Features

- **Multi-user multi-session** via the session broker — multiple RDP clients connect simultaneously, each user gets their own isolated desktop session
- **Live screen capture** via the ScreenCast XDG portal and PipeWire
- **H.264 streaming** via EGFX/AVC420 Dynamic Virtual Channel (10-50x bandwidth reduction vs raw bitmap, with automatic bitmap fallback for clients without EGFX support)
- **Keyboard and mouse injection** via reis/libei (direct libei protocol)
- **Clipboard sharing** (text) between local and remote sessions via CLIPRDR
- **Audio forwarding** from the desktop to the RDP client via RDPSND + PipeWire
- **Dynamic display resize** when the client window changes size
- **Cursor shape forwarding** (position, RGBA bitmap, hide/show)
- **Lock key synchronization** (Caps Lock, Num Lock, Scroll Lock state sync)
- **PAM authentication** via the session broker, with per-user session isolation
- **NLA authentication** via CredSSP (optional, for single-user mode)
- **TLS encryption** with self-signed certificates or user-provided PEM files
- **Hardware-accelerated encoding** with VAAPI (Intel/AMD) and NVENC (NVIDIA) support, automatic fallback to x264 software encoding
- **Session lifecycle management** with idle timeout, reconnection, and state persistence across broker restarts
- **COSMIC Settings GUI** for configuration management via D-Bus IPC
- **NixOS module** with systemd service, firewall integration, and secrets management
- **Home Manager module** for user-level installation
- **Graceful shutdown** on SIGINT/SIGTERM and D-Bus stop/reload commands
- **View-only fallback** when input injection is unavailable

## Architecture

### Crate overview

Workspace with 7 crates (v0.3.0):

| Crate | Purpose |
|-------|---------|
| `cosmic-ext-rdp-server` | Per-user daemon: CLI, config, TLS, D-Bus server, orchestration |
| `cosmic-ext-rdp-broker` | Multi-user session broker: TCP proxy, PAM auth, session lifecycle |
| `cosmic-ext-rdp-settings` | COSMIC Settings GUI: config editor, D-Bus status, nav pages |
| `rdp-dbus` | Shared D-Bus types, config structs, client/server proxy |
| `rdp-capture` | Screen capture via ScreenCast portal + PipeWire |
| `rdp-input` | Input injection via reis/libei (direct libei protocol) |
| `rdp-encode` | Video encoding via GStreamer (H.264) + bitmap fallback |

### Multi-user architecture

The session broker accepts all RDP connections on port 3389, authenticates users via PAM, and spawns isolated per-user `cosmic-ext-rdp-server` instances:

```
RDP Client A ──┐
RDP Client B ──┤  TCP :3389
RDP Client C ──┘
       │
       ▼
cosmic-ext-rdp-broker (system service, root)
       ├── Read X.224 Connection Request → extract cookie username
       ├── PAM authentication
       ├── Spawn per-user server via systemd-run
       └── TCP proxy (bidirectional byte-level forwarding)
            │
            ├── cosmic-ext-rdp-server :3390 (user A's session)
            ├── cosmic-ext-rdp-server :3391 (user B's session)
            └── cosmic-ext-rdp-server :3392 (user C's session)
```

Each per-user server inherits the user's environment (WAYLAND_DISPLAY, XDG_RUNTIME_DIR, DBUS_SESSION_BUS_ADDRESS) so all portals and PipeWire work transparently.

### Per-user data flow

```
RDP Client
    |
    v
cosmic-ext-rdp-server (per-user daemon)
    |
    |-- ScreenCast portal --> PipeWire --> rdp-capture --> rdp-encode --> EGFX H.264 or bitmap
    |-- RemoteDesktop portal --> EIS socket --> rdp-input --> compositor keyboard/mouse
    |-- CLIPRDR channel <--> arboard --> system clipboard
    |-- RDPSND channel <-- PipeWire audio monitor
    |-- D-Bus IPC <--> cosmic-ext-rdp-settings (GUI)
    v
ironrdp-server (RDP protocol)
```

### H.264 encoding pipeline

```
PipeWire (BGRx/BGRA) --> R/B swap --> GStreamer appsrc (BGRx, BT.709 full-range)
    --> videoconvert --> capsfilter (I420, BT.709 full-range)
    --> encoder (VAAPI/NVENC/x264) --> h264parse
    --> appsink (byte-stream, AU aligned)
    --> EGFX AVC420 PDU --> ZGFX compression --> DVC channel --> FreeRDP client
```

The encoder auto-detects hardware acceleration in priority order: VAAPI (Intel/AMD) > NVENC (NVIDIA) > x264 (software fallback).

### D-Bus interfaces

| Interface | Bus | Purpose |
|-----------|-----|---------|
| `io.github.olafkfreund.CosmicExtRdpBroker` | System | Session broker: list/terminate sessions, session count |
| `io.github.olafkfreund.CosmicExtRdpServer` | Session | Per-user daemon: status, reload, stop (settings GUI IPC) |
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
nix build .#cosmic-ext-rdp-settings     # Build settings GUI
nix build .#cosmic-ext-rdp-broker       # Build session broker
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
just build-broker-debug        # Build session broker (debug)
just build-broker-release      # Build session broker (release)
just check                     # Clippy with pedantic warnings
just run                       # Run server with RUST_BACKTRACE=full
just run-settings              # Run settings GUI
just run-broker                # Run session broker
just test                      # Run all workspace tests
just fmt                       # Format code
just clean                     # Clean build artifacts
sudo just install              # Install server to /usr/bin + desktop entry
sudo just install-settings     # Install settings GUI to /usr/bin + desktop entry
sudo just install-broker       # Install session broker to /usr/bin
sudo just install-all          # Install everything
```

### Building an AUR package (Arch Linux)

Create a `PKGBUILD`:

```bash
# Maintainer: Your Name <you@example.com>
pkgname=cosmic-ext-rdp-server
pkgver=0.3.0
pkgrel=1
pkgdesc="RDP server for the COSMIC Desktop Environment"
arch=('x86_64' 'aarch64')
url="https://github.com/olafkfreund/cosmic-ext-rdp-server"
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
Source: cosmic-ext-rdp-server
Section: net
Priority: optional
Maintainer: Your Name <you@example.com>
Build-Depends: debhelper-compat (= 13), cargo, rustc (>= 1.85),
 just, clang, pkg-config, libpipewire-0.3-dev, libei-dev,
 libwayland-dev, libxkbcommon-dev, libgstreamer1.0-dev,
 libgstreamer-plugins-base1.0-dev, libssl-dev, libfontconfig-dev,
 libfreetype-dev, libgl-dev, libvulkan-dev, libdbus-1-dev
Standards-Version: 4.7.0
Homepage: https://github.com/olafkfreund/cosmic-ext-rdp-server

Package: cosmic-ext-rdp-server
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
	just rootdir=debian/cosmic-ext-rdp-server install-all
```

**`debian/changelog`:**
```
cosmic-ext-rdp-server (0.3.0-1) unstable; urgency=medium

  * Multi-user multi-session broker (PAM auth, per-user isolation).
  * H.264 EGFX streaming with correct colors.
  * Hardware-accelerated encoding (VAAPI/NVENC).
  * COSMIC Settings GUI.
  * NixOS and Home Manager modules.

 -- Your Name <you@example.com>  Tue, 11 Feb 2026 00:00:00 +0000
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
cosmic-ext-rdp-server

# Specify a custom address and port
cosmic-ext-rdp-server --addr 0.0.0.0 --port 13389

# Use a custom TLS certificate
cosmic-ext-rdp-server --cert /path/to/cert.pem --key /path/to/key.pem

# Use a configuration file
cosmic-ext-rdp-server --config /path/to/config.toml

# Start with a static blue screen (for testing, no portal needed)
cosmic-ext-rdp-server --static-display
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
| `--swap-colors` | Force R/B channel swap (usually not needed, auto-detected) |

### Connecting from a client

```bash
# FreeRDP with H.264 EGFX (recommended - best quality and bandwidth)
xfreerdp /v:hostname:3389 /cert:ignore /gfx:avc420

# FreeRDP with NLA authentication
xfreerdp /v:hostname:3389 /u:myuser /p:mypassword /sec:nla /cert:ignore /gfx:avc420

# FreeRDP with dynamic resize
xfreerdp /v:hostname:3389 /cert:ignore /dynamic-resolution

# FreeRDP without EGFX (bitmap fallback)
xfreerdp /v:hostname:3389 /cert:ignore /gfx:off

# Remmina (Linux GUI)
# Create a new RDP connection, set Server to hostname:3389

# Windows Remote Desktop (mstsc.exe)
mstsc /v:hostname:3389
```

## Configuration

Configuration is read from TOML. Default location: `$XDG_CONFIG_HOME/cosmic-ext-rdp-server/config.toml` (`~/.config/cosmic-ext-rdp-server/config.toml`).

### Full example

```toml
# Network
bind = "0.0.0.0:3389"

# TLS (omit for self-signed)
# cert_path = "/etc/cosmic-ext-rdp-server/cert.pem"
# key_path = "/etc/cosmic-ext-rdp-server/key.pem"

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
swap_colors = true    # R/B channel swap for COSMIC portal (default: true)

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
| `swap_colors` | bool | `true` | Swap R/B channels (needed for COSMIC portal pixel format) |

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

### Session Broker Configuration

The multi-user session broker (`cosmic-ext-rdp-broker`) has its own TOML configuration. Default: `/etc/cosmic-ext-rdp-broker/config.toml`

```toml
bind = "0.0.0.0:3389"
server_binary = "/usr/bin/cosmic-ext-rdp-server"
port_range_start = 3390
port_range_end = 3489
pam_service = "cosmic-ext-rdp"
idle_timeout_secs = 3600
max_sessions = 100
session_policy = "OnePerUser"   # or "ReplaceExisting"
state_file = "/var/lib/cosmic-ext-rdp-broker/sessions.json"
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `bind` | string | `"0.0.0.0:3389"` | Address and port for the broker to listen on |
| `server_binary` | string | auto | Path to the `cosmic-ext-rdp-server` binary |
| `port_range_start` | int | `3390` | Start of the port range for per-user sessions |
| `port_range_end` | int | `3489` | End of the port range (supports up to 100 concurrent users) |
| `pam_service` | string | `"cosmic-ext-rdp"` | PAM service name for authentication |
| `idle_timeout_secs` | int | `3600` | Seconds of idle time before a session is terminated |
| `max_sessions` | int | `100` | Maximum number of concurrent user sessions |
| `session_policy` | string | `"OnePerUser"` | `OnePerUser` reconnects to existing sessions; `ReplaceExisting` terminates old sessions |
| `state_file` | string | see above | Path to the JSON session persistence file |

## Installation

### NixOS Module

The flake provides a NixOS module for declarative configuration.

#### Basic setup

```nix
{
  inputs.cosmic-ext-rdp-server.url = "github:olafkfreund/cosmic-ext-rdp-server";

  outputs = { self, nixpkgs, cosmic-ext-rdp-server, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        cosmic-ext-rdp-server.nixosModules.default
        {
          nixpkgs.overlays = [ cosmic-ext-rdp-server.overlays.default ];

          services.cosmic-ext-rdp-server = {
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
services.cosmic-ext-rdp-server = {
  enable = true;
  openFirewall = true;

  auth = {
    enable = true;
    username = "rdpuser";
    # Password is loaded via systemd LoadCredential (never in Nix store).
    # Compatible with agenix, sops-nix, or any file-based secrets manager.
    passwordFile = "/run/agenix/cosmic-ext-rdp-password";
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
| `package` | package | `pkgs.cosmic-ext-rdp-server` | Server package to use |
| `installSettings` | bool | `true` | Install the COSMIC Settings GUI |
| `settingsPackage` | package | `pkgs.cosmic-ext-rdp-settings` | Settings GUI package |
| `openFirewall` | bool | `false` | Open the RDP port in the firewall |
| `auth.enable` | bool | `false` | Enable NLA authentication |
| `auth.username` | string | `""` | NLA username |
| `auth.domain` | string | `null` | NLA domain (optional) |
| `auth.passwordFile` | path | `null` | Path to password file (loaded via `LoadCredential`) |
| `settings` | attrs | `{}` | TOML configuration (see Configuration section) |

The systemd service runs as a user service (`graphical-session.target`) with security hardening (no new privileges, read-only home, private tmp, restricted syscalls).

#### Multi-user broker setup (NixOS)

For multi-user remote desktop access, enable the session broker alongside the per-user server:

```nix
services.cosmic-ext-rdp-broker = {
  enable = true;
  openFirewall = true;

  settings = {
    bind = "0.0.0.0:3389";
    port_range_start = 3390;
    port_range_end = 3489;
    max_sessions = 100;
    idle_timeout_secs = 3600;
  };
};
```

The broker runs as a system service (root) to perform PAM authentication and spawn per-user sessions via `systemd-run`. It automatically configures PAM and creates a systemd slice (`cosmic-ext-rdp-sessions.slice`) with resource limits (8 GB memory, 4096 tasks) for all RDP sessions combined.

### Home Manager Module

For user-level installation without system-wide NixOS changes.

#### Basic setup

```nix
{
  inputs.cosmic-ext-rdp-server.url = "github:olafkfreund/cosmic-ext-rdp-server";

  outputs = { self, nixpkgs, home-manager, cosmic-ext-rdp-server, ... }: {
    homeConfigurations."user" = home-manager.lib.homeManagerConfiguration {
      modules = [
        cosmic-ext-rdp-server.homeManagerModules.default
        {
          nixpkgs.overlays = [ cosmic-ext-rdp-server.overlays.default ];

          services.cosmic-ext-rdp-server = {
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
services.cosmic-ext-rdp-server = {
  enable = true;
  autoStart = true;

  auth = {
    enable = true;
    username = "rdpuser";
    passwordFile = "/run/agenix/cosmic-ext-rdp-password";
  };

  settings.bind = "0.0.0.0:3389";
};
```

#### Home Manager options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | `false` | Enable the COSMIC RDP Server |
| `package` | package | `pkgs.cosmic-ext-rdp-server` | Server package to use |
| `installSettings` | bool | `true` | Install the COSMIC Settings GUI |
| `settingsPackage` | package | `pkgs.cosmic-ext-rdp-settings` | Settings GUI package |
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
| RDP Client | --> | cosmic-ext-rdp-server | --> | xdg-desktop-portal-cosmic   |
| (mstsc,    |     | (this repo)       |     | (RemoteDesktop + ScreenCast)|
| FreeRDP,   | <-- | RDP protocol,     | <-- | EIS socket pairs,          |
| Remmina)   |     | TLS, auth         |     | PipeWire streams            |
+------------+     +-------------------+     +-----------------------------+
```

| Component | Repository | Purpose |
|-----------|-----------|---------|
| [cosmic-ext-rdp-server](https://github.com/olafkfreund/cosmic-ext-rdp-server) | This repo | RDP protocol server, capture + input orchestration |
| [xdg-desktop-portal-cosmic](https://github.com/olafkfreund/xdg-desktop-portal-cosmic) | Portal fork | RemoteDesktop + ScreenCast portal interfaces |
| [cosmic-comp-rdp](https://github.com/olafkfreund/cosmic-comp-rdp) | Compositor fork | EIS receiver for input injection |

### How the components interact

1. **cosmic-ext-rdp-server** starts and calls the **RemoteDesktop** portal to request input injection and the **ScreenCast** portal to request screen capture
2. **xdg-desktop-portal-cosmic** shows a consent dialog, creates a UNIX socket pair for EIS, and sends the server-side fd to the compositor
3. **cosmic-comp-rdp** receives the EIS socket via D-Bus (`AcceptEisSocket`) and creates a seat with keyboard, pointer, and touch capabilities
4. The RDP server receives the client-side EIS socket via `ConnectToEIS` and sends input events through it
5. PipeWire streams carry screen frames from the compositor to the RDP server for encoding and delivery

### NixOS example (all three components)

```nix
{
  inputs = {
    cosmic-ext-rdp-server.url = "github:olafkfreund/cosmic-ext-rdp-server";
    xdg-desktop-portal-cosmic.url = "github:olafkfreund/xdg-desktop-portal-cosmic";
    cosmic-comp.url = "github:olafkfreund/cosmic-comp-rdp";
  };

  outputs = { self, nixpkgs, cosmic-ext-rdp-server, xdg-desktop-portal-cosmic, cosmic-comp, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        cosmic-ext-rdp-server.nixosModules.default
        xdg-desktop-portal-cosmic.nixosModules.default
        cosmic-comp.nixosModules.default
        {
          nixpkgs.overlays = [
            cosmic-ext-rdp-server.overlays.default
            xdg-desktop-portal-cosmic.overlays.default
            cosmic-comp.overlays.default
          ];

          # Compositor with EIS support
          services.cosmic-comp.enable = true;

          # Portal with RemoteDesktop interface
          services.xdg-desktop-portal-cosmic.enable = true;

          # RDP server
          services.cosmic-ext-rdp-server = {
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
    cosmic-ext-rdp-server.url = "github:olafkfreund/cosmic-ext-rdp-server";
    xdg-desktop-portal-cosmic.url = "github:olafkfreund/xdg-desktop-portal-cosmic";
    cosmic-comp.url = "github:olafkfreund/cosmic-comp-rdp";
  };

  outputs = { self, nixpkgs, home-manager, cosmic-ext-rdp-server, xdg-desktop-portal-cosmic, cosmic-comp, ... }: {
    homeConfigurations."user" = home-manager.lib.homeManagerConfiguration {
      modules = [
        cosmic-ext-rdp-server.homeManagerModules.default
        xdg-desktop-portal-cosmic.homeManagerModules.default
        cosmic-comp.homeManagerModules.default
        {
          nixpkgs.overlays = [
            cosmic-ext-rdp-server.overlays.default
            xdg-desktop-portal-cosmic.overlays.default
            cosmic-comp.overlays.default
          ];

          services.cosmic-ext-rdp-server = {
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

| Dependency | cosmic-ext-rdp-server | xdg-desktop-portal-cosmic | cosmic-comp-rdp |
|------------|-------------------|---------------------------|-----------------|
| reis (libei) | 0.5 | 0.5 | 0.5 |
| zbus (D-Bus) | 5.x | 5.x | 5.x |
| ashpd (portals) | 0.12 | 0.12 | - |
| pipewire | 0.8 | git (freedesktop) | - |

D-Bus interface chain (verified compatible):
- Portal exposes `org.freedesktop.impl.portal.RemoteDesktop` with `ConnectToEIS`
- Portal calls `com.system76.CosmicComp.RemoteDesktop.AcceptEisSocket(fd)` on the compositor
- RDP server exposes `io.github.olafkfreund.CosmicExtRdpServer` for settings GUI IPC

## D-Bus Interface

**Per-user daemon** (`io.github.olafkfreund.CosmicExtRdpServer` on the session bus):

- **Properties:** `Status` (Running/Stopped/Error), `BindAddress`
- **Methods:** `Reload`, `Stop`
- **Signals:** Status change notifications

The settings GUI (`cosmic-ext-rdp-settings`) communicates with the daemon over this interface to display server status and trigger configuration reloads.

**Session broker** (`io.github.olafkfreund.CosmicExtRdpBroker` on the system bus):

- **Methods:** `ListSessions` (returns all active sessions), `TerminateSession(username)`, `ActiveSessionCount`

The broker's D-Bus interface can be used for monitoring and administration of multi-user sessions.

## Logging

The server uses `tracing` with `RUST_LOG` environment variable support:

```bash
# Default (info level)
cosmic-ext-rdp-server

# Debug logging
RUST_LOG=debug cosmic-ext-rdp-server

# Trace logging for specific crates
RUST_LOG=rdp_capture=trace,rdp_input=debug cosmic-ext-rdp-server
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
- Check logs: `RUST_LOG=rdp_input=debug cosmic-ext-rdp-server`

### Connection refused

- Check the server is running: `systemctl --user status cosmic-ext-rdp-server`
- Check firewall rules: port 3389 (or custom port) must be open
- For NixOS: set `openFirewall = true` in the module configuration

### Wrong colors (red/blue swapped)

- The `swap_colors` option defaults to `true` for COSMIC Desktop
- If colors appear inverted, try setting `swap_colors = false` in `[capture]`
- This is needed because COSMIC's portal reports BGRx format but delivers RGBx byte order

### Audio not working

- Ensure PipeWire is running with audio support
- Check `[audio] enable = true` in the configuration
- Ensure the RDP client supports RDPSND (FreeRDP does by default)

## Known Limitations

- **Dynamic resize:** Resize during an active EGFX session may trigger a reconnection loop; bitmap-mode resize works correctly
- **Cursor shapes:** SPA cursor metadata extraction requires unsafe FFI not yet implemented; cursor position is forwarded but custom cursor bitmaps from PipeWire are stubbed
- **Unicode input:** Full IME/compose input is not yet supported ([#23](https://github.com/olafkfreund/cosmic-ext-rdp-server/issues/23)); common control characters (Backspace, Tab, Enter, Escape, Delete) sent as Unicode events are handled

## License

GPL-3.0-only
