# AGENTS.md

## Purpose
This file is the dedicated place for coding-agent instructions for this repository (a “README for agents”).

## Project overview
zAgent is a Rust workspace that provides a multi-LLM CLI, a backend server, and multiple frontends with tool execution, observability, and session storage.

## Repo layout
- `crates/zagent-core/`: Core library (agent loop, providers, tools, sessions)
- `crates/zagent-native/`: Native CLI binary (REPL, SurrealDB, tracing, native tools)
- `crates/zagent-backend/`: Shared backend engine + HTTP API router
- `crates/zagent-server/`: Axum-based backend server binary
- `crates/zagent-tui/`: Terminal UI frontend (ratatui)
- `crates/zagent-web/`: Web/WASM frontend (ratzilla + trunk)
- `crates/zagent-ui-shared/`: Shared UI models/state for TUI and web
- `crates/zagent-wasi/`: WASI component scaffold
- `wit/`: WIT interface definitions for tools
- `logs/`: Runtime logs (do not edit)

## Setup / common commands
- Build workspace: `cargo build`
- Run tests: `cargo test`
- Check formatting: `cargo fmt --all --check`
- Lint (if configured): `cargo clippy --workspace --all-targets -- -D warnings`

Run individual binaries:
- Native CLI: `cargo run -p zagent-native -- --help`
- Backend server: `cargo run -p zagent-server -- --help`
- TUI frontend: `cargo run -p zagent-tui -- --help`

## Expected agent behavior
- **Inspect before changing**: read relevant files first; avoid blind edits.
- **Small, focused diffs**: prefer minimal changes that fully solve the task.
- **Keep docs in sync**: if behavior/interfaces change, update relevant docs/comments.
- **Run programmatic checks**: if instructions list tests/lints, run the relevant ones and fix failures before finishing.
- **Don’t touch generated / runtime artifacts**: avoid editing `logs/`, `dist/`, `target/`, built WASM bundles, or other generated files.
- **Prefer existing patterns**: follow the project’s module structure and naming.
- **Avoid dependency churn** unless it’s required for the task.

## Testing / quality bar
- If you change code, **run relevant checks** (at minimum `cargo test`).
- If tests fail, **fix them before finishing** (or clearly explain what’s failing and why).
- When adding/changing behavior, **add or update tests** where it’s practical.

## Code style
- Follow standard Rust formatting (`cargo fmt`).
- Keep public APIs documented and avoid breaking changes unless explicitly requested.
- Prefer clear error messages and structured error types where the codebase already does.

## Environment
- Requires Rust **1.85+** (Rust 2024 edition).
- Uses `.env` for API keys (see `.env.example`).
