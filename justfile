set shell := ["bash", "-cu"]

export CARGO := "cargo"
export TRUNK := "trunk"
export RUSTUP := "rustup"
export BACKEND_PORT := "8787"
export BACKEND_URL := "http://127.0.0.1:8787"
export SURREALDB_HOST := "127.0.0.1"
export SURREALDB_PORT := "8000"
export SURREALDB_URL := "ws://127.0.0.1:8000"
export SURREALDB_USER := "root"
export SURREALDB_PASS := "root"
export SURREALDB_NS := "zagent"
export SURREALDB_DB := "session_storage"
export SURREALDB_CONTAINER := "zagent-surrealdb-dev"
export DIOXUS := "dx"
export DIOXUS_WEB_PORT := "8080"
export WEB_DEMO_PORT := "8081"

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

# Run the standalone embedded wasm demo page.
[group('Web')]
run-web-demo:
    command -v "$TRUNK" >/dev/null 2>&1 || { echo "$TRUNK is required; run 'just trunk-install' first"; exit 1; }
    cd crates/zagent-web-demo && $TRUNK serve --port "$WEB_DEMO_PORT"

# Run the standalone embedded wasm demo page and open a browser automatically.
[group('Web')]
run-web-demo-open:
    command -v "$TRUNK" >/dev/null 2>&1 || { echo "$TRUNK is required; run 'just trunk-install' first"; exit 1; }
    cd crates/zagent-web-demo && $TRUNK serve --port "$WEB_DEMO_PORT" --open

# Run SurrealDB, zagent-server, and the Dioxus web app together for local development.
[group('Web')]
[parallel]
run-dioxus-dev: run-dioxus-db run-dioxus-server run-dioxus-web

[private]
run-dioxus-db:
    #!/usr/bin/env bash
    set -euo pipefail

    surrealdb_url="ws://$SURREALDB_HOST:$SURREALDB_PORT"
    echo "Starting SurrealDB on $surrealdb_url"

    if command -v surreal >/dev/null 2>&1; then
        exec surreal start \
            --bind "$SURREALDB_HOST:$SURREALDB_PORT" \
            --user "$SURREALDB_USER" \
            --pass "$SURREALDB_PASS" \
            memory
    fi

    if command -v docker >/dev/null 2>&1; then
        docker rm -f "$SURREALDB_CONTAINER" >/dev/null 2>&1 || true
        trap 'docker rm -f "$SURREALDB_CONTAINER" >/dev/null 2>&1 || true' EXIT INT TERM
        exec docker run --rm \
            --name "$SURREALDB_CONTAINER" \
            -p "$SURREALDB_HOST:$SURREALDB_PORT:8000" \
            surrealdb/surrealdb:latest \
            start \
            --bind "0.0.0.0:8000" \
            --user "$SURREALDB_USER" \
            --pass "$SURREALDB_PASS" \
            memory
    fi

    echo "SurrealDB requires either the 'surreal' CLI or Docker"
    exit 1

[private]
run-dioxus-server:
    #!/usr/bin/env bash
    set -euo pipefail

    surrealdb_url="ws://$SURREALDB_HOST:$SURREALDB_PORT"
    backend_url="http://127.0.0.1:$BACKEND_PORT"

    for _ in {1..50}; do
        if (echo >/dev/tcp/"$SURREALDB_HOST"/"$SURREALDB_PORT") >/dev/null 2>&1; then
            echo "Starting zagent-server on $backend_url"
            exec env SURREALDB_URL="$surrealdb_url" $CARGO run -p zagent-server -- --port "$BACKEND_PORT"
        fi
        sleep 0.2
    done

    echo "SurrealDB did not become ready on $surrealdb_url"
    exit 1

[private]
run-dioxus-web:
    #!/usr/bin/env bash
    set -euo pipefail

    command -v "$DIOXUS" >/dev/null 2>&1 || { echo "$DIOXUS is required"; exit 1; }
    echo "Starting Dioxus web app with $DIOXUS serve"
    cd crates/zagent-dioxus
    exec "$DIOXUS" serve --platform web --package web --port "$DIOXUS_WEB_PORT" --open false

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
