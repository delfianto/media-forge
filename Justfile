# Justfile for Media Forge

set shell := ["zsh", "-c"]

export RUSTFLAGS := "-C target-cpu=native"

default: build

# Build the release binary
build:
    cargo build --release

# Run the release binary with arguments
run +args:
    cargo run --release -- {{args}}

# Check code quality (clippy, fmt, tests)
check:
    cargo check
    cargo clippy -- -D warnings
    cargo fmt --all -- --check
    cargo test

# Install to /usr/local/bin (requires sudo)
install: build
    @echo "Installing media-forge to /usr/local/bin..."
    sudo install -m 755 -o root -g root target/release/media-forge /usr/local/bin/media-forge
    @echo "Installation complete. Verify with 'media-forge --help'"

# Uninstall from /usr/local/bin
uninstall:
    @echo "Removing media-forge from /usr/local/bin..."
    sudo rm -f /usr/local/bin/media-forge

# Clean build artifacts
clean:
    cargo clean
