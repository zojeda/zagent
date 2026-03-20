set shell := ["bash", "-cu"]

export CARGO := "cargo"
export TRUNK := "trunk"
export RUSTUP := "rustup"
export BACKEND_PORT := "8787"
export BACKEND_URL := "http://127.0.0.1:{{BACKEND_PORT}}"

alias run-native-repl := run-native

# Show available recipes.
[group('General')]
help:
    @just --list --unsorted

# Build the workspace in debug mode.
[group('Build')]
build:
    $CARGO build

# Build the workspace in release mode.
[group('Build')]
build-release:
    $CARGO build --release

# Build the native CLI with OpenTelemetry enabled.
[group('Build')]
otel-build:
    $CARGO build -p zagent-native --features otel

# Run the test suite.
[group('Check')]
test:
    $CARGO test

# Format the workspace.
[group('Check')]
fmt:
    $CARGO fmt --all

# Check formatting without changing files.
[group('Check')]
fmt-check:
    $CARGO fmt --all --check

# Run clippy across the workspace.
[group('Check')]
clippy:
    $CARGO clippy --workspace --all-targets -- -D warnings

# Run the standard local verification suite.
[group('Check')]
check: fmt-check test clippy

# Run the native CLI in REPL mode.
[group('Run')]
run-native:
    $CARGO run -p zagent-native

# Run the native CLI with PROMPT="...".
[group('Run')]
run-native-prompt:
    test -n "${PROMPT:-}" || { echo "PROMPT is required"; exit 1; }
    $CARGO run -p zagent-native -- --prompt "$PROMPT"

# Run the backend server with the native runtime.
[group('Run')]
run-server:
    $CARGO run -p zagent-server -- --port "$BACKEND_PORT"

# Run the backend server with the native runtime and OTEL enabled.
[group('Run')]
run-server-otel:
    $CARGO run -p zagent-server --features otel -- --port "$BACKEND_PORT"

# Run the backend server with the WASI runtime.
[group('Run')]
run-server-wasi:
    $CARGO run -p zagent-server -- --runtime wasi --port "$BACKEND_PORT"

# Run the terminal frontend against the configured backend.
[group('Run')]
run-tui:
    $CARGO run -p zagent-tui -- --backend-url "$BACKEND_URL"

# Add the wasm32 target needed for the web frontend.
[group('Web')]
wasm-target:
    $RUSTUP target add wasm32-unknown-unknown

# Install trunk for web development.
[group('Web')]
trunk-install:
    $CARGO install trunk

# Run the combined backend + trunk web dev workflow.
[group('Web')]
run-web-dev:
    $CARGO run -p zagent-web-dev -- --open

# Run trunk manually, assuming the backend is already running.
[group('Web')]
run-web-manual:
    cd crates/zagent-web && $TRUNK serve --proxy-backend "$BACKEND_URL/api" --proxy-rewrite /api

# Print the newest log file.
[group('Logs')]
logs-today:
    log_file=$$(ls -1 logs/zagent.log.* 2>/dev/null | tail -n 1); test -n "$$log_file" || { echo "No log files found in logs/"; exit 1; }; echo "==> $$log_file"; cat "$$log_file"

# Print tool-call entries from the newest log file.
[group('Logs')]
logs-tool-calls:
    command -v jq >/dev/null 2>&1 || { echo "jq is required"; exit 1; }
    log_file=$$(ls -1 logs/zagent.log.* 2>/dev/null | tail -n 1); test -n "$$log_file" || { echo "No log files found in logs/"; exit 1; }; echo "==> $$log_file"; grep "tool_call" "$$log_file" | jq .

# Run the native CLI with OTEL enabled.
[group('Observability')]
otel-run:
    test -n "${PROMPT:-}" || { echo "PROMPT is required"; exit 1; }
    test -n "${OTEL_EXPORTER_OTLP_ENDPOINT:-}" || { echo "OTEL_EXPORTER_OTLP_ENDPOINT is required"; exit 1; }
    $CARGO run -p zagent-native --features otel -- --prompt "$PROMPT"
