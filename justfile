name := 'cosmic-ext-rdp-server'
settings-name := 'cosmic-ext-rdp-settings'
broker-name := 'cosmic-ext-rdp-broker'
export APPID := 'io.github.olafkfreund.CosmicExtRdpServer'
export SETTINGS_APPID := 'io.github.olafkfreund.CosmicExtRdpSettings'
export BROKER_APPID := 'io.github.olafkfreund.CosmicExtRdpBroker'

rootdir := ''
prefix := '/usr'

base-dir := absolute_path(clean(rootdir / prefix))
bin-dir := base-dir / 'bin'
share-dir := base-dir / 'share'

# Default recipe: build release
default: build-release

# Build debug binary
build-debug *args:
    cargo build {{args}}

# Build release binary
build-release *args:
    cargo build --release {{args}}

# Build settings app (debug)
build-settings-debug *args:
    cargo build -p cosmic-ext-rdp-settings {{args}}

# Build settings app (release)
build-settings-release *args:
    cargo build --release -p cosmic-ext-rdp-settings {{args}}

# Build broker (debug)
build-broker-debug *args:
    cargo build -p cosmic-ext-rdp-broker {{args}}

# Build broker (release)
build-broker-release *args:
    cargo build --release -p cosmic-ext-rdp-broker {{args}}

# Run clippy with pedantic warnings
check *args:
    cargo clippy --workspace --all-targets -- -W clippy::pedantic {{args}}

# Run with backtrace enabled
run *args:
    RUST_BACKTRACE=full cargo run -- {{args}}

# Run settings app
run-settings *args:
    RUST_BACKTRACE=full cargo run -p cosmic-ext-rdp-settings -- {{args}}

# Run broker
run-broker *args:
    RUST_BACKTRACE=full cargo run -p cosmic-ext-rdp-broker -- {{args}}

# Run tests
test *args:
    cargo test --workspace {{args}}

# Format code
fmt:
    cargo fmt --all

# Format check
fmt-check:
    cargo fmt --all -- --check

# Clean build artifacts
clean:
    cargo clean

# Install server to system
install:
    install -Dm0755 target/release/{{name}} {{bin-dir}}/{{name}}
    install -Dm0644 data/{{APPID}}.desktop {{share-dir}}/applications/{{APPID}}.desktop

# Install settings app to system
install-settings:
    install -Dm0755 target/release/{{settings-name}} {{bin-dir}}/{{settings-name}}
    install -Dm0644 data/{{SETTINGS_APPID}}.desktop {{share-dir}}/applications/{{SETTINGS_APPID}}.desktop

# Install broker to system
install-broker:
    install -Dm0755 target/release/{{broker-name}} {{bin-dir}}/{{broker-name}}

# Install everything
install-all: install install-settings install-broker

# Uninstall server from system
uninstall:
    rm -f {{bin-dir}}/{{name}}
    rm -f {{share-dir}}/applications/{{APPID}}.desktop

# Uninstall settings from system
uninstall-settings:
    rm -f {{bin-dir}}/{{settings-name}}
    rm -f {{share-dir}}/applications/{{SETTINGS_APPID}}.desktop

# Uninstall broker from system
uninstall-broker:
    rm -f {{bin-dir}}/{{broker-name}}

# Uninstall everything
uninstall-all: uninstall uninstall-settings uninstall-broker
