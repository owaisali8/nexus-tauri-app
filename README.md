# Essentio

Essentio is a Tauri 2 desktop application for creating local-first AI agents, storing notes/files, and preparing browser automation workflows.

## Current Capabilities

- React + TypeScript desktop UI with a dark Nothing-inspired theme.
- Rust core module boundaries for agents, tools, skills, memory, LLMs, MCP, browser runs, and persistence.
- SQLite state persisted in the Tauri app data directory.
- LLM providers configured from the UI (API keys and base URLs stored locally).
- Custom agent creation with system instructions, provider, and model selection.
- Session-based chat interface with persisted message history.
- File records and long-term agent memory tables.
- Saved notes and browser run drafts.
- Rig-backed LLM prompt execution through Tauri commands.

## LLM Providers

Providers are configured from the **Providers** view in the app. Credentials are stored in the local SQLite database.

Environment variables are still supported as a fallback when a provider has not been saved in the UI:

- OpenAI: `OPENAI_API_KEY`, optional `OPENAI_BASE_URL`
- OpenRouter: `OPENROUTER_API_KEY`
- Anthropic: `ANTHROPIC_API_KEY`
- Ollama: optional `OLLAMA_API_BASE_URL`, optional `OLLAMA_API_KEY`
- LM Studio: optional `LMSTUDIO_BASE_URL`, optional `LMSTUDIO_API_KEY`

Default local URLs:

- Ollama: `http://localhost:11434`
- LM Studio: `http://localhost:1234/v1`

## Development

Install dependencies:

```bash
npm install
```

Run the desktop app:

```bash
npm run tauri dev
```

Build frontend:

```bash
npm run build
```

Check Rust:

```bash
cd src-tauri
cargo check
```

Run Rust tests:

```bash
cd src-tauri
cargo test
```

## Persistence

The app stores local data in `essentio.sqlite3` under the Tauri app data directory. Migrations live in `src-tauri/db/migrations`.

On first launch after upgrading from the early JSON prototype, Essentio imports `essentio-state.json` if it exists and the SQLite database is empty.

## Next Slices

- CDP browser controller for navigation, DOM inspection, field filling, and PDF upload.
- MCP server config, discovery, and tool invocation.
- Agent skills loaded from local folders and injected into Rig context.
