import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  type EngineEvent,
  type EngineKind,
  type ModelInfo,
  type ProviderView,
  type Session,
  type Usage,
  createSession,
  deleteSession,
  getMessages,
  listModels,
  listProviders,
  listSessions,
  renameSession,
  runStream,
} from "./lib/ipc";
import { ProviderSettings } from "./features/settings/ProviderSettings";
import { WindowControls } from "./WindowControls";
import "./App.css";

type Turn = {
  id: string;
  role: "system" | "user" | "assistant";
  content: string;
  pending?: boolean;
};

type ConnectionState =
  | { status: "idle" }
  | { status: "testing" }
  | { status: "ok"; models: ModelInfo[] }
  | { status: "failed"; message: string };

function newId() {
  return crypto.randomUUID();
}

/** First line of the opening message, trimmed to something list-sized. */
function deriveTitle(text: string) {
  const firstLine = text.trim().split("\n")[0] ?? "";
  return firstLine.length > 48 ? `${firstLine.slice(0, 48)}…` : firstLine;
}

function formatWhen(unixSeconds: number) {
  const date = new Date(unixSeconds * 1000);
  const sameDay = new Date().toDateString() === date.toDateString();
  return sameDay
    ? date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
    : date.toLocaleDateString([], { month: "short", day: "numeric" });
}

export default function App() {
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [providerId, setProviderId] = useState("");
  const [model, setModel] = useState("");
  const [engine, setEngine] = useState<EngineKind>("direct");
  const [connection, setConnection] = useState<ConnectionState>({
    status: "idle",
  });

  const [showSettings, setShowSettings] = useState(false);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSessionId, setActiveSessionId] = useState("");
  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [usage, setUsage] = useState<Usage | null>(null);

  const cancelRef = useRef<(() => void) | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const activeProvider = useMemo(
    () => providers.find((provider) => provider.id === providerId) ?? null,
    [providers, providerId],
  );

  const refreshProviders = useCallback(async () => {
    try {
      const loaded = await listProviders();
      setProviders(loaded);
      // Keep the current selection if it still exists; otherwise fall back to
      // the first provider, or clear it when the last one was removed.
      setProviderId((current) =>
        loaded.some((provider) => provider.id === current)
          ? current
          : (loaded[0]?.id ?? ""),
      );
    } catch (error: unknown) {
      setConnection({ status: "failed", message: String(error) });
    }
  }, []);

  const refreshSessions = useCallback(async () => {
    try {
      setSessions(await listSessions());
    } catch (error: unknown) {
      console.error("failed to load sessions", error);
    }
  }, []);

  useEffect(() => {
    void refreshProviders();
    void refreshSessions();
  }, [refreshProviders, refreshSessions]);

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
    if (providerId) {
      void testConnection(providerId);
    } else {
      setConnection({
        status: "failed",
        message: "No provider configured. Add one from Providers.",
      });
    }
  }, [providerId, testConnection]);

  useEffect(() => {
    scrollRef.current?.scrollTo({
      top: scrollRef.current.scrollHeight,
      behavior: "smooth",
    });
  }, [turns]);

  useEffect(() => () => cancelRef.current?.(), []);

  const openSession = useCallback(
    async (session: Session) => {
      cancelRef.current?.();
      cancelRef.current = null;
      setIsStreaming(false);
      setUsage(null);
      setActiveSessionId(session.id);
      // A session is pinned to the engine that produced it, so replies stay
      // consistent with the transcript above them.
      setEngine(session.engine);

      try {
        const messages = await getMessages(session.id);
        setTurns(
          messages.map((message) => ({
            id: message.id,
            role: message.role,
            content: message.content,
          })),
        );
      } catch (error: unknown) {
        setTurns([]);
        console.error("failed to load messages", error);
      }
    },
    [],
  );

  const startNewChat = useCallback(() => {
    cancelRef.current?.();
    cancelRef.current = null;
    setIsStreaming(false);
    setActiveSessionId("");
    setTurns([]);
    setUsage(null);
    setInput("");
  }, []);

  const removeSession = useCallback(
    async (sessionId: string) => {
      try {
        await deleteSession(sessionId);
        if (sessionId === activeSessionId) startNewChat();
        await refreshSessions();
      } catch (error: unknown) {
        console.error("failed to delete session", error);
      }
    },
    [activeSessionId, refreshSessions, startNewChat],
  );

  const stop = useCallback(() => {
    cancelRef.current?.();
    cancelRef.current = null;
    setIsStreaming(false);
    setTurns((current) => current.map((turn) => ({ ...turn, pending: false })));
    void refreshSessions();
  }, [refreshSessions]);

  const send = useCallback(async () => {
    const text = input.trim();
    if (!text || isStreaming || !providerId || !model) return;

    // The session is created on first send rather than on "New chat", so
    // abandoned drafts never litter the conversation list.
    let sessionId = activeSessionId;
    if (!sessionId) {
      try {
        const session = await createSession({
          title: deriveTitle(text),
          providerId,
          model,
          engine,
        });
        sessionId = session.id;
        setActiveSessionId(session.id);
        await refreshSessions();
      } catch (error: unknown) {
        setTurns((current) => [
          ...current,
          { id: newId(), role: "assistant", content: `⚠ ${String(error)}` },
        ]);
        return;
      }
    }

    const assistantId = newId();
    setTurns((current) => [
      ...current,
      { id: newId(), role: "user", content: text },
      { id: assistantId, role: "assistant", content: "", pending: true },
    ]);
    setInput("");
    setIsStreaming(true);
    setUsage(null);

    const settle = (patch: Partial<Turn> = {}) => {
      cancelRef.current = null;
      setIsStreaming(false);
      setTurns((current) =>
        current.map((turn) =>
          turn.id === assistantId ? { ...turn, pending: false, ...patch } : turn,
        ),
      );
      void refreshSessions();
    };

    cancelRef.current = runStream(
      { sessionId, providerId, model, prompt: text, temperature: 0.7, engine },
      (event: EngineEvent) => {
        switch (event.type) {
          case "token":
            setTurns((current) =>
              current.map((turn) =>
                turn.id === assistantId
                  ? { ...turn, content: turn.content + event.text }
                  : turn,
              ),
            );
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
  }, [
    input,
    isStreaming,
    providerId,
    model,
    engine,
    activeSessionId,
    refreshSessions,
  ]);

  const canSend = Boolean(input.trim() && !isStreaming && providerId && model);

  return (
    <div className="shell">
      <header className="titlebar" data-tauri-drag-region>
        <span className="titlebar__name">Essentio</span>
        <WindowControls />
      </header>

      <div className="body">
        <aside className="sidebar">
          <button
            type="button"
            className="button button--send sidebar__new"
            onClick={startNewChat}
          >
            New chat
          </button>

          <nav className="sidebar__list">
            {sessions.length === 0 && (
              <p className="sidebar__empty">No conversations yet.</p>
            )}
            {sessions.map((session) => (
              <div
                key={session.id}
                className={`session ${
                  session.id === activeSessionId ? "session--active" : ""
                }`}
              >
                <button
                  type="button"
                  className="session__open"
                  onClick={() => void openSession(session)}
                  onDoubleClick={() => {
                    const next = window.prompt("Rename chat", session.title);
                    if (next?.trim()) {
                      void renameSession(session.id, next.trim()).then(
                        refreshSessions,
                      );
                    }
                  }}
                  title={session.title}
                >
                  <span className="session__title">{session.title}</span>
                  <span className="session__meta">
                    {session.engine === "adk" ? "ADK" : "Direct"} ·{" "}
                    {formatWhen(session.updatedAt)}
                  </span>
                </button>
                <button
                  type="button"
                  className="session__delete"
                  aria-label={`Delete ${session.title}`}
                  onClick={() => void removeSession(session.id)}
                >
                  ×
                </button>
              </div>
            ))}
          </nav>
        </aside>

        <div className="main">
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
                // Locked once a session exists: its transcript was produced by
                // one engine and only that engine has the matching context.
                disabled={isStreaming || Boolean(activeSessionId)}
                onChange={(event) => setEngine(event.target.value as EngineKind)}
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
                disabled={!providerId}
                onClick={() => providerId && void testConnection(providerId)}
              >
                Test
              </button>
              <button
                type="button"
                className="button button--ghost"
                onClick={() => setShowSettings(true)}
              >
                Providers
              </button>
            </div>
          </div>

          <main className="messages" ref={scrollRef}>
            {connection.status === "failed" && (
              <div className="notice notice--error">
                <strong>
                  Can’t reach {activeProvider?.label ?? "provider"}
                </strong>
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
                  void send();
                }
              }}
            />
            {isStreaming ? (
              <button
                type="button"
                className="button button--stop"
                onClick={stop}
              >
                Stop
              </button>
            ) : (
              <button
                type="button"
                className="button button--send"
                disabled={!canSend}
                onClick={() => void send()}
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
      </div>

      {showSettings && (
        <ProviderSettings
          onClose={() => setShowSettings(false)}
          onChanged={() => void refreshProviders()}
        />
      )}
    </div>
  );
}
