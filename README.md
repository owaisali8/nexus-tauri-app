# Essentio

A local-first AI workspace. Tauri 2 desktop shell, React 19 frontend, Rust core.

Chat runs against local model servers (LM Studio, Ollama, any OpenAI-compatible
endpoint) or cloud providers, with conversations stored on your machine.

## Status

Phase 1 complete: chat, persistence, and provider management work end to end.
Tools, MCP, RAG and research are not built yet.

| Area | State |
|---|---|
| Streaming chat | works |
| LM Studio / Ollama / OpenAI-compatible | verified live |
| Google Gemini | verified live |
| OpenAI / DeepSeek | implemented, not verified against the live API |
| Anthropic | implemented, **not verified against the live API** |
| Conversation persistence | works, survives restart |
| Markdown + syntax highlighting | works |
| Regenerate, edit-and-resend | works |
| Tools / MCP / RAG / research | not started |

## Architecture

```
core/          engine-agnostic product logic
  engine/      AgentEngine trait + EngineEvent — the one seam engines cross
    adk/       ADK-Rust implementation (the only place adk_* may be imported)
    direct/    framework-free implementation over the provider transports
  providers/   ChatTransport per wire format: openai_compat, anthropic, gemini
  memory/      SQLite store for sessions, messages, settings
tauri-app/     shell: commands, streaming channel, OS keychain
frontend/      React 19 + TypeScript
```

Two engines implement the same `AgentEngine` trait and are selectable per
conversation:

- **Direct** — streamed completions, no agent framework.
- **ADK** — ADK-Rust's agent loop and session handling, driven through our own
  transports rather than its built-in ones. See `core/src/engine/adk/model.rs`
  for why.

`core/tests/adk_boundary.rs` fails the build if any `adk_*` reference appears
outside `core/src/engine/adk/`, which is what keeps the engine swappable.

## Secrets

API keys live in the **OS keychain** (Windows Credential Manager, macOS
Keychain, Secret Service on Linux). Provider config on disk stores only the
name of the keychain entry, never the secret. No key is written to the
database, to logs, or returned to the frontend.

## Requirements

- Rust (edition 2024; built with 1.96)
- Node 20+
- A model source: LM Studio or Ollama running locally, or an API key

## Development

```bash
# frontend deps
npm --prefix frontend install

# run the app — from the workspace root, not frontend/
./frontend/node_modules/.bin/tauri dev
```

The Tauri CLI only searches subdirectories for `tauri.conf.json`, so it has to
run from the workspace root.

### Gates

All of these must pass before a commit:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

npm --prefix frontend run typecheck
npm --prefix frontend run lint
npm --prefix frontend run build
```

### Live checks

These need LM Studio running and are excluded from `cargo test`:

```bash
cargo run -p essentio-core --example lmstudio_smoke   # direct transport
cargo run -p essentio-core --example adk_smoke        # ADK engine + persistence
```

`adk_smoke` also guards a regression where ADK's own transport silently
discarded everything after the first `<` or `[` in a reply.

## Data locations

- Database: `<app data>/workspace.sqlite3`
- Provider config: `<app data>/providers.json`
- API keys: OS keychain, service `com.owais.essentio`

On Windows `<app data>` is `%APPDATA%\com.owais.essentio`.
