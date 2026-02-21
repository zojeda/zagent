# zAgent Makefile

SHELL := /bin/sh

CARGO ?= cargo
TRUNK ?= trunk
RUSTUP ?= rustup

BACKEND_PORT ?= 8787
BACKEND_URL ?= http://127.0.0.1:$(BACKEND_PORT)

.PHONY: help build build-release run-native run-native-repl run-native-prompt \
	run-server run-server-wasi run-tui wasm-target trunk-install run-web-dev \
	run-web-manual logs-today logs-tool-calls otel-build otel-run

help:
	@echo "Available targets:"
	@echo "  build               Build debug binaries"
	@echo "  build-release       Build release binaries"
	@echo "  run-native          Run native CLI (REPL)"
	@echo "  run-native-repl     Alias for run-native"
	@echo "  run-native-prompt   Run native CLI with PROMPT=\"...\""
	@echo "  run-server          Run backend server (native runtime)"
	@echo "  run-server-otel     Run backend server (native runtime) OTEL"
	@echo "  run-server-wasi     Run backend server (WASI runtime)"
	@echo "  run-tui             Run terminal frontend"
	@echo "  wasm-target         Add wasm32-unknown-unknown target"
	@echo "  trunk-install       Install trunk (cargo install trunk)"
	@echo "  run-web-dev         Run web dev (ratzilla + trunk proxy)"
	@echo "  run-web-manual      Run trunk serve (expects backend already running)"
	@echo "  logs-today          Show today's log file contents"
	@echo "  logs-tool-calls     Show today's tool call logs (requires jq)"
	@echo "  otel-build          Build native CLI with OpenTelemetry feature"
	@echo "  otel-run            Run native CLI with OTEL_EXPORTER_OTLP_ENDPOINT"

build:
	$(CARGO) build

build-release:
	$(CARGO) build --release

run-native:
	$(CARGO) run -p zagent-native

run-native-repl: run-native

run-native-prompt:
	@test -n "$(PROMPT)" || (echo "PROMPT is required" && exit 1)
	$(CARGO) run -p zagent-native -- -p "$(PROMPT)"

run-server:
	$(CARGO) run -p zagent-server -- --port $(BACKEND_PORT)

run-server-otel:
	$(CARGO) run -p zagent-server -F otel -- --port $(BACKEND_PORT)

run-server-wasi:
	$(CARGO) run -p zagent-server -- --runtime wasi --port $(BACKEND_PORT)

run-tui:
	$(CARGO) run -p zagent-tui -- --backend-url $(BACKEND_URL)

wasm-target:
	$(RUSTUP) target add wasm32-unknown-unknown

trunk-install:
	$(CARGO) install trunk

run-web-dev:
	$(CARGO) run -p zagent-web-dev -- --open

run-web-manual:
	cd crates/zagent-web && $(TRUNK) serve --proxy-backend $(BACKEND_URL)/api --proxy-rewrite /api

logs-today:
	@ls -1 logs/zagent.log.* | tail -n 1 | xargs -I{} sh -c 'echo "==> {}"; cat {}'

logs-tool-calls:
	@ls -1 logs/zagent.log.* | tail -n 1 | xargs -I{} sh -c 'echo "==> {}"; grep "tool_call" {} | jq .'

otel-build:
	$(CARGO) build -p zagent-native --features otel

otel-run:
	@test -n "$(PROMPT)" || (echo "PROMPT is required" && exit 1)
	@test -n "$(OTEL_EXPORTER_OTLP_ENDPOINT)" || (echo "OTEL_EXPORTER_OTLP_ENDPOINT is required" && exit 1)
	$(CARGO) run -p zagent-native --features otel -- --prompt "$(PROMPT)"
