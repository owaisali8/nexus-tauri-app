# Nexus

A local-first AI workspace. Tauri 2 desktop shell, React 19 frontend, Rust core.

Chat runs against local model servers (LM Studio, Ollama, any OpenAI-compatible
endpoint) or cloud providers, with conversations stored on your machine.

## Status

Chat, persistence, provider management, tool calling, MCP and document search
work end to end. Deep research and the documents editor are not built.

"Verified live" below means exercised against the real service or a real
server, not merely unit-tested.

| Area | State |
|---|---|
| Streaming chat | verified live |
| LM Studio / Ollama / OpenAI-compatible | verified live |
| Google Gemini | verified live, including tool calling |
| OpenAI / DeepSeek | implemented, not verified against the live API |
| Anthropic | implemented, **not verified**, and cannot carry tools |
| Conversation persistence | verified, survives restart |
| Markdown + syntax highlighting | works |
| Regenerate, edit-and-resend | works |
| Agents (named instruction + model + tool profiles) | verified live |
| Tool calling + approval gate | verified live |
| Built-in tools (`current_time`, `write_note`) | verified live |
| MCP servers | verified live, incl. through chat |
| Document search (RAG) | verified live, local embeddings |
| Deep research / documents editor / compare | not started |

## Document search

Text files are chunked, embedded and stored in the same SQLite database as
everything else, then compared by brute-force cosine. LanceDB — the original
plan — took the dependency tree from 753 to 2157 crates for a corpus that will
hold thousands of chunks, not millions. Past roughly 100k chunks this wants a
real index; `Retriever` is narrow enough to swap then.

Retrieval is a tool the model calls, not context injected into every message,
so a conversation that has nothing to do with your files does not pay for a
search.

Embedding similarity has a high floor — measured against `nomic-embed-text`,
unrelated text still scores 0.41–0.51 where genuinely relevant passages score
0.64–0.71. An absolute cutoff therefore cannot separate the two. Weak matches
are returned but flagged, so the model can say your documents do not cover
something rather than citing the closest paragraph as though they did.

## Agents

An agent is a saved profile: system instructions, provider, model,
temperature, and which tools it may use. A conversation either belongs to an
agent or is plain chat, which stays a first-class path rather than a stripped
down agent.

Deleting an agent leaves its conversations intact and unattached. They are
the user's, not the profile's.

## Tools and approval

A tool declares whether it is read-only or side-effecting. Side-effecting
calls are held until you approve them; an unanswered prompt times out to
*deny*, and cancelling a run denies whatever it left waiting.

**Every MCP tool is treated as side-effecting**, including ones a server marks
read-only. That marking is self-attestation from third-party code, and
honouring it would let a server opt itself out of the only check between it
and your machine. The cost is a prompt per call; the fix is per-tool trust you
grant, which is not built yet.

MCP servers are configured in `mcp.json` and launched as child processes.
Command names are resolved through `PATHEXT`, so `npx` works on Windows where
a plain PATH search would not find the `.cmd` shim.

## Architecture

```
core/          engine-agnostic product logic
  engine/      AgentEngine trait + EngineEvent — the one seam engines cross
    direct/    streamed completions and a tool loop, no agent framework
  providers/   ChatTransport per wire format: openai_compat, anthropic, gemini
  rag/         chunking, embeddings, retrieval
  memory/      SQLite store for sessions, messages, agents, documents
  tools/       Tool trait, approval gate, built-ins, MCP client
tauri-app/     shell: commands, streaming channel, OS keychain, approval router
frontend/      React 19 + TypeScript
```

### No agent framework

This ran on [ADK-Rust](https://github.com/zavora-ai/adk-rust) for a while and
no longer does. The decision is worth recording, because "add a framework" is
the obvious move and it did not pay here.

By the time ADK worked, every part of it had been replaced:

- Its transport dropped every character after the first `<` or `[` in a reply,
  so `Vec<T>` truncated an answer. Replaced with our own.
- Its tool loop was never wired; ours does the work.
- Its sessions are in-memory, so state was rehydrated from SQLite each run.
- `Runner::run` does not create a missing session despite documenting that it
  does, and the failure was silent.

What remained was a `Runner` wrapping our transport, emitting events we mapped
back to our own type — for 52% of the dependency tree (755 crates down to 364
on removal).

`AgentEngine` stays. It is what made trying ADK cheap and dropping it cheaper,
and it is where a framework goes if one earns its place. Multi-agent
orchestration with handoff and shared state would be a real reason; a
plan → search → read → synthesize loop is not.

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
npm install                    # Tauri CLI, at the workspace root
npm --prefix frontend install  # frontend deps

npm run dev                    # run the app
npm run build                  # bundle it
```

Both from the workspace root. The Tauri CLI lives there rather than in
`frontend/` because it only searches *subdirectories* for `tauri.conf.json`,
and because it treats the directory holding `package.json` as the app root —
which is what `beforeDevCommand`'s paths are relative to.

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
cargo run -p nexus-core --example lmstudio_smoke   # direct transport
cargo run -p nexus-core --example tools_smoke      # tool call -> execute -> answer
```

This one needs `npx` rather than LM Studio, and downloads a server on first
run:

```bash
cargo run -p nexus-core --example mcp_smoke        # real MCP server round trip
```

RAG needs an embedding model loaded in LM Studio:

```bash
cargo run -p nexus-core --example rag_smoke        # ingest, retrieve, rank
```

## Data locations

- Database: `<app data>/workspace.sqlite3`
- Provider config: `<app data>/providers.json`
- MCP servers: `<app data>/mcp.json`
- Notes written by `write_note`: `<app data>/notes/`
- API keys: OS keychain, service `com.owais.nexus`

On Windows `<app data>` is `%APPDATA%\com.owais.nexus`.
