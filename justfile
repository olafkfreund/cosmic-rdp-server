name := 'cosmic-rdp-server'
export APPID := 'com.system76.CosmicRdpServer'

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

# Run clippy with pedantic warnings
check *args:
    cargo clippy --workspace --all-targets -- -W clippy::pedantic {{args}}

# Run with backtrace enabled
run *args:
    RUST_BACKTRACE=full cargo run -- {{args}}

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

# Install to system
install:
    install -Dm0755 target/release/{{name}} {{bin-dir}}/{{name}}
    install -Dm0644 data/{{APPID}}.desktop {{share-dir}}/applications/{{APPID}}.desktop

# Uninstall from system
uninstall:
    rm -f {{bin-dir}}/{{name}}
    rm -f {{share-dir}}/applications/{{APPID}}.desktop
