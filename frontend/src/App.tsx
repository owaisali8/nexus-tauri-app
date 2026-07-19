import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  type ChatMessage,
  type EngineEvent,
  type EngineKind,
  type ModelInfo,
  type ProviderView,
  type Usage,
  listModels,
  listProviders,
  runStream,
} from "./lib/ipc";
import { WindowControls } from "./WindowControls";
import "./App.css";

type Turn = ChatMessage & { id: string; pending?: boolean };

type ConnectionState =
  | { status: "idle" }
  | { status: "testing" }
  | { status: "ok"; models: ModelInfo[] }
  | { status: "failed"; message: string };

function newId() {
  return crypto.randomUUID();
}

export default function App() {
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [providerId, setProviderId] = useState("");
  const [model, setModel] = useState("");
  const [connection, setConnection] = useState<ConnectionState>({
    status: "idle",
  });

  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [usage, setUsage] = useState<Usage | null>(null);
  const [engine, setEngine] = useState<EngineKind>("direct");

  // Conversation state lives in the Rust engine keyed by this id. Changing
  // engines starts a fresh session, since the two do not share history.
  const [sessionId, setSessionId] = useState(() => crypto.randomUUID());

  const cancelRef = useRef<(() => void) | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const activeProvider = useMemo(
    () => providers.find((provider) => provider.id === providerId) ?? null,
    [providers, providerId],
  );

  useEffect(() => {
    listProviders()
      .then((loaded) => {
        setProviders(loaded);
        if (loaded[0]) setProviderId(loaded[0].id);
      })
      .catch((error: unknown) => {
        setConnection({ status: "failed", message: String(error) });
      });
  }, []);

  const testConnection = useCallback(async (id: string) => {
    setConnection({ status: "testing" });
    try {
      const models = await listModels(id);
      setConnection({ status: "ok", models });
      const firstChat = models.find((item) => !item.id.includes("embed"));
      setModel((current) => current || firstChat?.id || models[0]?.id || "");
    } catch (error: unknown) {
      setConnection({ status: "failed", message: String(error) });
    }
  }, []);

  useEffect(() => {
    if (providerId) void testConnection(providerId);
  }, [providerId, testConnection]);

  useEffect(() => {
    scrollRef.current?.scrollTo({
      top: scrollRef.current.scrollHeight,
      behavior: "smooth",
    });
  }, [turns]);

  // Cancel any in-flight run if the view unmounts.
  useEffect(() => () => cancelRef.current?.(), []);

  const stop = useCallback(() => {
    cancelRef.current?.();
    cancelRef.current = null;
    setIsStreaming(false);
    setTurns((current) => current.map((turn) => ({ ...turn, pending: false })));
  }, []);

  const send = useCallback(() => {
    const text = input.trim();
    if (!text || isStreaming || !providerId || !model) return;

    const userTurn: Turn = { id: newId(), role: "user", content: text };
    const assistantId = newId();

    setTurns((current) => [
      ...current,
      userTurn,
      { id: assistantId, role: "assistant", content: "", pending: true },
    ]);
    setInput("");
    setIsStreaming(true);
    setUsage(null);

    const appendToAssistant = (chunk: string) =>
      setTurns((current) =>
        current.map((turn) =>
          turn.id === assistantId
            ? { ...turn, content: turn.content + chunk }
            : turn,
        ),
      );

    const settle = (patch: Partial<Turn> = {}) => {
      cancelRef.current = null;
      setIsStreaming(false);
      setTurns((current) =>
        current.map((turn) =>
          turn.id === assistantId ? { ...turn, pending: false, ...patch } : turn,
        ),
      );
    };

    cancelRef.current = runStream(
      { sessionId, providerId, model, prompt: text, temperature: 0.7, engine },
      (event: EngineEvent) => {
        switch (event.type) {
          case "token":
            appendToAssistant(event.text);
            break;
          case "done":
            setUsage(event.usage ?? null);
            settle();
            break;
          case "error":
            settle({ content: `⚠ ${event.message}` });
            break;
          default:
            // tool_call / tool_result / citation land in Phase 2.
            break;
        }
      },
    );
  }, [input, isStreaming, providerId, model, sessionId, engine]);

  const canSend = Boolean(input.trim() && !isStreaming && providerId && model);

  return (
    <div className="shell">
      <header className="titlebar" data-tauri-drag-region>
        <span className="titlebar__name">Essentio</span>
        <WindowControls />
      </header>

      <div className="toolbar">
        <label className="field">
          <span className="field__label">Provider</span>
          <select
            className="field__control"
            value={providerId}
            onChange={(event) => {
              setProviderId(event.target.value);
              setModel("");
            }}
          >
            {providers.map((provider) => (
              <option key={provider.id} value={provider.id}>
                {provider.label}
              </option>
            ))}
          </select>
        </label>

        <label className="field">
          <span className="field__label">Model</span>
          <select
            className="field__control"
            value={model}
            disabled={connection.status !== "ok"}
            onChange={(event) => setModel(event.target.value)}
          >
            {connection.status === "ok" ? (
              connection.models.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.id}
                </option>
              ))
            ) : (
              <option value="">—</option>
            )}
          </select>
        </label>

        <label className="field">
          <span className="field__label">Engine</span>
          <select
            className="field__control field__control--narrow"
            value={engine}
            disabled={isStreaming}
            onChange={(event) => {
              setEngine(event.target.value as EngineKind);
              // Engines keep separate history, so switching starts a new
              // conversation rather than silently losing context.
              setSessionId(crypto.randomUUID());
              setTurns([]);
              setUsage(null);
            }}
          >
            <option value="direct">Direct</option>
            <option value="adk">ADK</option>
          </select>
        </label>

        <div className="toolbar__status">
          {connection.status === "testing" && (
            <span className="badge badge--muted">Connecting…</span>
          )}
          {connection.status === "ok" && (
            <span className="badge badge--ok">
              {connection.models.length} model
              {connection.models.length === 1 ? "" : "s"}
            </span>
          )}
          {connection.status === "failed" && (
            <span className="badge badge--error" title={connection.message}>
              Offline
            </span>
          )}
          <button
            type="button"
            className="button button--ghost"
            onClick={() => providerId && void testConnection(providerId)}
          >
            Test
          </button>
        </div>
      </div>

      <main className="messages" ref={scrollRef}>
        {connection.status === "failed" && (
          <div className="notice notice--error">
            <strong>Can’t reach {activeProvider?.label ?? "provider"}</strong>
            <p>{connection.message}</p>
          </div>
        )}

        {turns.length === 0 && connection.status === "ok" && (
          <div className="empty">
            <h1 className="empty__title">Local and ready</h1>
            <p className="empty__body">
              Streaming through {activeProvider?.label}. Nothing leaves this
              machine.
            </p>
          </div>
        )}

        {turns.map((turn) => (
          <article key={turn.id} className={`turn turn--${turn.role}`}>
            <div className="turn__role">
              {turn.role === "user" ? "You" : "Assistant"}
            </div>
            <div className="turn__body">
              {turn.content}
              {turn.pending && !turn.content && (
                <span className="caret" aria-label="waiting" />
              )}
            </div>
          </article>
        ))}
      </main>

      <footer className="composer">
        <textarea
          className="composer__input"
          value={input}
          rows={1}
          placeholder={
            connection.status === "ok"
              ? "Send a message…"
              : "Start your local server to begin"
          }
          disabled={connection.status !== "ok"}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              send();
            }
          }}
        />
        {isStreaming ? (
          <button type="button" className="button button--stop" onClick={stop}>
            Stop
          </button>
        ) : (
          <button
            type="button"
            className="button button--send"
            disabled={!canSend}
            onClick={send}
          >
            Send
          </button>
        )}
      </footer>

      {usage && (
        <div className="usage">
          {usage.promptTokens} in · {usage.completionTokens} out ·{" "}
          {usage.totalTokens} total
        </div>
      )}
    </div>
  );
}
