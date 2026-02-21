# zAgent

A Codex CLI alternative built in Rust, focused on **multi-LLM provider support**, **full observability**, and **coding automation** through shell and file tools.

## Features

- **Multi-LLM Provider Architecture** — Trait-based provider system with OpenRouter as the default backend, giving access to models from Anthropic, OpenAI, Google, Meta, and more
- **Full Observability** — See exactly what is sent to the LLM, every tool call and its result, with dual-layer tracing (colored terminal + structured JSON log files)
- **Agentic Tool Loop** — The agent autonomously executes shell commands, reads/writes files, and lists directories to accomplish tasks
- **Session Persistence** — SurrealDB-backed session store to save and resume conversations
- **Interactive REPL & Single-Shot Mode** — Use as a conversational assistant or pipe in one-off prompts
- **Multi-Frontend Architecture** — Shared backend server with both `ratatui` (terminal) and `ratzilla` (web/WASM) frontends
- **Dual Runtime Support** — Native runtime with full tools or WASI runtime with restricted sandboxed execution
- **WASI Component Model Ready** — WIT interface definitions for host-injected tools, enabling sandboxed WebAssembly execution
- **Workspace Instructions** — Automatically discovers `AGENTS.md` files and injects their guidance into the system prompt

## Quick Start

### Prerequisites

- Rust 1.85+ (2024 edition)
- An [OpenRouter](https://openrouter.ai/) API key

### Installation

```bash
git clone https://github.com/your-user/zAgent.git
cd zAgent
cp .env.example .env
# Edit .env and add your OPENROUTER_API_KEY
cargo build --release
```

### Usage

#### Usage Examples

**Summarize a repository (single-shot):**

```bash
cargo run -p zagent-native -- -p "Summarize the codebase in the current directory and list key entry points"
```

**Generate and run a Rust script:**

```bash
cargo run -p zagent-native -- -p "Create src/bin/hello.rs that prints Hello from zAgent, then run it"
```

**Refactor with a specific working directory:**

```bash
cargo run -p zagent-native -- -w ./crates/zagent-core -- -p "Refactor error.rs to use thiserror and update call sites"
```

**Use a custom system prompt:**

```bash
cargo run -p zagent-native -- -s "You are a strict Rust reviewer" -p "Review src/lib.rs and suggest improvements"
```

#### Native CLI

**Single-shot prompt:**

```bash
# Binary will be at target/release/zagent after build
cargo run -p zagent-native -- -p "Create a hello world Python script in /tmp/hello.py and run it"
```

**Interactive REPL:**

```bash
cargo run -p zagent-native
```

**Choose a model:**

```bash
cargo run -p zagent-native -- -m openai/gpt-4o -p "Explain the Rust borrow checker"
cargo run -p zagent-native -- -m google/gemini-2.5-pro -p "Refactor this function"
cargo run -p zagent-native -- -m minimax/minimax-m2.5 -p "Write unit tests for src/lib.rs"
```

#### Backend + Frontends

**Start the backend server:**

```bash
# Native runtime (full tools + SurrealDB sessions)
cargo run -p zagent-server -- --port 8787

# WASI runtime (restricted tools + JSON sessions)
cargo run -p zagent-server -- --runtime wasi --port 8787
```

**Terminal frontend (ratatui):**

```bash
cargo run -p zagent-tui -- --backend-url http://127.0.0.1:8787
```

**Web frontend (ratzilla + WASM):**

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
cargo run -p zagent-web-dev -- --open
```

This command starts both services and configures Trunk to proxy `/api/*` to the backend server.

Manual equivalent (if you want separate terminals):

```bash
cargo run -p zagent-server -- --port 8787
cd crates/zagent-web
trunk serve --proxy-backend http://127.0.0.1:8787/api --proxy-rewrite /api
```

### Runtime Capabilities

- **Native**: SurrealDB session store + full toolset (`shell_exec`, `file_read`, `file_write`, `file_edit`, `list_dir`, `websearch`, `webfetch`)
- **WASI**: JSON session store + restricted toolset (`file_read`, `file_write`, `file_edit`, `list_dir`, `websearch`, `webfetch`) - no shell access

### Workspace Instructions (`AGENTS.md`)

zAgent automatically scans for `AGENTS.md` files starting from the working directory, walking up to the git root and down into subdirectories (skipping `target/`, `node_modules/`, `dist/`, and `logs/`). Discovered instructions are appended to the system prompt in path order with these rules:

- Closer, more specific `AGENTS.md` files take precedence over broader ones.
- Up to 64 instruction files are loaded, each capped at 32 KB.
- Explicit user chat instructions always override `AGENTS.md` guidance.

### Custom Agents (`.agents`)

Define specialized child agents in `.agents/*.md` (or `.agents/*.agent.md`) with optional frontmatter:

```md
---
name: Rust specialist
description: Handles Rust refactors and tests
model: minimax/minimax-m2.5
user-invokable: true
invoke-default: false
tools: ["search", "fetch"]
handoffs:
  - label: Start Implementation
    agent: Implementation Agent
    prompt: Now implement the plan outlined above.
    send: false
    model: minimax/minimax-m2.5
---
You are a focused Rust coding agent. Keep answers concise and code-first.
```

Each file becomes a dynamic handoff tool (`handoff_<agent-name-slug>`). Handoffs run in isolated child sessions to avoid parent-context pollution, and traces are nested under the calling tool span with handoff spans named `agent_handoff <child-agent-name>`.

`user-invokable: true` enables explicit routing from user prompts with `@agent-name ...` at the start.  
`invoke-default: true` (only valid when `user-invokable` is true) auto-routes normal user prompts to that agent when no explicit `@...` prefix is used.

Optional manifest fields:

- `tools`: runtime tool access policy (enforced). This filters both:
  - tool definitions exposed to the model
  - runtime execution (blocked if not allowed)
- `handoffs`: list of allowed downstream agents with optional overrides:
  - `label`: display label for the handoff.
  - `agent`: target agent name.
  - `prompt`: additional prompt appended to the handoff request.
  - `send`: set `false` to prevent sending `context` to the child agent.
  - `model`: override the child agent model for this handoff.

Tool policy patterns:

- Exact name: `file_read`, `websearch`, `shell_exec`
- Wildcard: `*` (all tools), `file/*` (normalized to `file_*`)
- Regex: `re:^file_(read|write)$`
- Aliases:
  - `search` -> `websearch`
  - `fetch` -> `webfetch`
  - `read_fs` / `read` -> `file_read`, `list_dir`
  - `write_fs` / `write` -> `file_write`, `file_edit`
  - `filesystem` / `fs` -> `file_read`, `file_write`, `file_edit`, `list_dir`
  - `vcs` / `git` / `version_control` -> `shell_exec`, `file_read`, `list_dir`

Manifest constraints:

- `id` is not allowed in custom-agent manifests; identity is derived from `name`.
- `handoffs[].agent` must reference agent `name`.
- The effective available runtime tools are injected into the system prompt for each active agent loop.

### Frontend Controls

- `Enter`: send prompt
- `Ctrl+Enter`: newline in input
- `Ctrl+T`: toggle inline tool result details in conversation
- `Tab`: cycle focused panel (`Conversation` -> `Feedback`)
- `Up/Down`: scroll focused panel
- `PageUp/PageDown`: fast scroll focused panel
- `Home/End`: jump to top/bottom of focused panel
- `Ctrl+Shift+C`: copy current input to clipboard
- `Ctrl+Shift+V`: paste clipboard into input
- `Ctrl+C` (TUI): press once to arm quit, press again within 2s to exit (any other key cancels)
- Feedback panel shows a live status ticker plus scrollable activity history
- Real-time prompt execution feedback and incremental assistant output via streaming endpoint

### Runtime Management

Runtime can be changed over HTTP with `POST /api/runtime` and body:

```json
{ "runtime": "native" }
```

### Slash Commands

Available in both server/frontends and native CLI:

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/model [provider/model]` | Show or set current model |
| `/runtime [native\|wasi]` | Show or switch active runtime |
| `/session list` | List saved sessions |
| `/session new [name]` | Start a new session |
| `/session continue <name-or-id>` | Resume a session |

## CLI Reference

```
Usage: zagent [OPTIONS]

Options:
  -p, --prompt <PROMPT>              Single-shot prompt (skips REPL)
  -m, --model <MODEL>                Model to use [default: minimax/minimax-m2.5]
  -s, --system <SYSTEM>              Custom system prompt
  -w, --working-dir <DIR>            Working directory for tool execution
      --log-dir <DIR>                JSON log directory [default: ./logs]
      --session-dir <DIR>            Session database directory
  -S, --session <NAME_OR_ID>         Resume a session by name or ID
      --new-session <NAME>           Start a new named session
      --list-sessions                List all sessions and exit
      --delete-session <NAME_OR_ID>  Delete a session by name or ID
  -v, --verbose                      Verbose mode (TRACE-level terminal output)
      --max-turns <N>                Maximum agent turns per invocation [default: 50]
  -h, --help                         Print help
  -V, --version                      Print version
```

### REPL Commands (Native CLI)

| Command     | Description                    |
|-------------|--------------------------------|
| `/help`     | Show available commands        |
| `/session`  | Show current session info      |
| `/sessions` | List all saved sessions        |
| `/clear`    | Clear conversation history     |
| `/quit`     | Exit the REPL                  |

## Architecture

```
zAgent/
├── crates/
│   ├── zagent-core/          # Core library (provider-agnostic)
│   │   └── src/
│   │       ├── agent/        # Agentic loop & conversation management
│   │       ├── provider/     # Provider trait & OpenRouter implementation
│   │       ├── session/      # Session types & SessionStore trait
│   │       ├── tools/        # Tool trait & ToolRegistry
│   │       └── error.rs      # Unified error types
│   ├── zagent-native/        # Native CLI binary (reqwest, SurrealDB, REPL)
│   │   └── src/
│   │       ├── tools/        # shell_exec, file_read, file_write, file_edit, list_dir, websearch, webfetch
│   │       ├── main.rs       # CLI entry point & REPL
│   │       ├── platform.rs   # reqwest HTTP client
│   │       ├── session_store.rs  # SurrealDB session store
│   │       └── tracing_setup.rs  # Multi-layer tracing
│   ├── zagent-backend/       # Shared backend engine + HTTP API server router
│   │   └── src/
│   │       ├── api.rs        # HTTP API routes
│   │       ├── engine.rs     # Backend agent engine 
│   │       ├── runtime.rs    # Runtime switching (native/wasi)
│   │       ├── session_store.rs     # SurrealDB session store
│   │       ├── session_store_json.rs  # JSON session store (WASI)
│   │       └── tools/        # Backend tool implementations
│   ├── zagent-server/        # Backend server binary (axum)
│   ├── zagent-tui/           # ratatui terminal frontend
│   ├── zagent-web/           # ratzilla WASM frontend
│   ├── zagent-web-dev/       # Dev helper to run backend + web with proxy
│   ├── zagent-ui-shared/     # Reusable UI models/state helpers shared by TUI/Web
│   └── zagent-wasi/          # WASI Component Model target (scaffold)
├── wit/
│   └── agent-tools.wit       # WIT interface definitions
├── .env.example
└── Cargo.toml                # Workspace root
```

### Core Abstractions

**Provider Trait** — Defines how to communicate with any OpenAI-compatible API:

```rust
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn base_url(&self) -> &str;
    fn api_key(&self) -> &str;
    fn auth_headers(&self) -> Vec<(String, String)>;
    fn extra_headers(&self) -> Vec<(String, String)>;
    fn map_model_name(&self, model: &str) -> String;
    fn supports_tools(&self) -> bool;
    fn prepare_request(&self, request: ChatRequest) -> ChatRequest;
    fn parse_response(&self, body: &str) -> Result<ChatResponse>;
    fn build_http_request(&self, chat_request: &ChatRequest) -> Result<HttpRequest>;
    // ...
}
```

**Tool Trait** — Defines executable tools the agent can invoke:

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    async fn execute(&self, args: serde_json::Value) -> Result<String>;
}
```

### Built-in Tools

| Tool | Description | Available In |
|------|-------------|--------------|
| `shell_exec` | Execute shell commands with configurable timeout (default 60s) | Native runtime only |
| `file_read` | Read file contents, optionally by line range | Both runtimes |
| `file_write` | Write content to files, creating parent directories as needed | Both runtimes |
| `file_edit` | Apply unified diff hunks to existing files for surgical edits | Both runtimes |
| `list_dir` | List directory contents with optional recursion (max depth 3) | Both runtimes |
| `websearch` | Search the web for top links/snippets (DuckDuckGo Instant Answer API) | Both runtimes |
| `webfetch` | Fetch URL content and return trimmed text (HTML cleaned to plain text) | Both runtimes |

## Observability

zAgent provides three layers of observability:

### 1. Terminal Output

Colored, human-readable output showing agent actions in real time. Use `--verbose` for TRACE-level detail including full API request/response payloads.

### 2. JSON Log Files

Structured TRACE-level logs written to daily rolling files in `--log-dir` (default: `./logs/`). By default, transport internals (`hyper`, `h2`, `reqwest`, `tower`, `tonic`, `rustls`) are filtered out so logs focus on model calls, tool usage, and agent decisions:

```bash
# View today's log
cat logs/zagent.log.2026-02-16

# Search for tool calls
grep "tool_call" logs/zagent.log.2026-02-16 | jq .
```

### 3. OpenTelemetry (Optional)

Enable with the `otel` feature flag for distributed tracing via OTLP/gRPC:

```bash
cargo build -p zagent-native --features otel
```

Start Jaeger with OTLP enabled and run zAgent:

```bash
# Start Jaeger (OTLP gRPC on 4317, UI on 16686)
docker run --rm -p 16686:16686 -p 4317:4317 \
  jaegertracing/all-in-one:1.57

# In another shell, enable OTLP and run zAgent
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
export OTEL_SERVICE_NAME=zagent
cargo run -p zagent-native --features otel -- \
  --prompt "Hello from zAgent with OTLP tracing"

# Open http://localhost:16686 to view traces
```

OTEL traces include a top-level `agent_session` span that encloses all turns in a session, with nested `model_call` and `tool_call` spans for the high-signal workflow.

### Session Tracking

Each session records:
- Full conversation history (all messages)
- Tool execution records with arguments, results, success/failure, and latency
- Token usage (prompt + completion tokens)
- Timestamps and metadata

## Sessions

Sessions are persisted to SurrealDB (native) or JSON files (WASI) and can be resumed across invocations:

```bash
# Start a named session
cargo run -p zagent-native -- --new-session my-project

# Resume it later
cargo run -p zagent-native -- --session my-project

# List all sessions
cargo run -p zagent-native -- --list-sessions

# Delete a session
cargo run -p zagent-native -- --delete-session my-project
```

In server/frontend mode, sessions are managed via slash commands or HTTP API.

## Session Synchronization

zAgent uses Server-Sent Events (SSE) with a central broadcast mechanism to synchronize conversation events between multiple clients connected to the same session.

### Core Components

1. **StreamHub** (`crates/zagent-backend/src/api.rs`)
   - Uses tokio's `broadcast::Sender<StreamChunk>` with a 1024-event buffer
   - Maintains a circular history buffer (4096 events) for reconnection support
   - Each event has a sequence number for ordering and replay

2. **Session State** - Stored in SurrealDB via `SurrealSessionStore`
   - Contains `messages`, `tool_executions`, and `SessionMeta` (id, name, model, etc.)

### Data Flow

```
┌──────────────────────────┐
│  Client A & Client B     │  ← Two SSE connections to /api/events/stream
└────────────┬─────────────┘
             │ SSE
             ▼
┌──────────────────────────┐
│  StreamHub               │  ← broadcast::Sender sends to ALL subscribers
│  (shared event bus)      │
└────────────┬─────────────┘
             │ publish()
             ▼
┌──────────────────────────┐
│  BackendEngine           │  ← run_message_turn() publishes events:
│  (single engine)         │    status, submit, events, delta, final
└────────────┬─────────────┘
             │
             ▼
┌──────────────────────────┐
│  zagent-core             │  ← Agent loop emits UiEvent
│  (agent loop)            │    (ModelRequestStarted, ToolCallStarted, etc.)
└──────────────────────────┘
```

### Why Both Clients Receive the Same Events

| Mechanism | How it works |
|-----------|--------------|
| **Shared StreamHub** | Single `broadcast::Sender` instance serves all clients |
| **SSE Subscription** | Clients subscribe via `stream_hub.subscribe()` - all get the same channel |
| **Publish to All** | When `stream_hub.publish()` is called, **every** subscriber receives the event |
| **History Replay** | On connect, client can request events since their last sequence number |

The architecture is designed for **single-server, single-active-session** operation. The `run_lock` Mutex in StreamHub ensures events are published in order, preventing race conditions between concurrent message processing.

## Configuration

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `OPENROUTER_API_KEY` | Yes | Your OpenRouter API key |
| `RUST_LOG` | No | Override tracing filter (e.g., `zagent=debug`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | No | OTLP/gRPC endpoint for OpenTelemetry (requires `--features otel`) |
| `OTEL_SERVICE_NAME` | No | Service name for OTLP traces (default: `zagent`) |

Copy `.env.example` to `.env` to configure:

```bash
cp .env.example .env
```

## Adding a New Provider

Implement the `Provider` trait and register it:

```rust
use zagent_core::provider::{Provider, ChatRequest, ChatResponse, HttpRequest};

pub struct MyProvider { /* ... */ }

impl Provider for MyProvider {
    fn name(&self) -> &str { "my-provider" }
    fn base_url(&self) -> &str { "https://api.my-provider.com/v1" }
    fn api_key(&self) -> &str { &self.api_key }
    // ... implement remaining methods or use defaults
}
```

## WASI Component Model

WIT interfaces are defined in `wit/agent-tools.wit` for running zAgent as a sandboxed WebAssembly component. The host provides tool implementations (shell, file I/O, sessions) via the Component Model, enabling:

- Sandboxed execution with capability-based security
- Host-controlled tool access
- Portable deployment across WASI-compatible runtimes

The `zagent-wasi` crate is scaffolded but not yet implemented. The WASI runtime in `zagent-backend` provides a preview of this functionality with restricted tool access.

## License

MIT
