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
  listTools,
  renameSession,
  respondToApproval,
  runStream,
  truncateSession,
  type ToolSpec,
} from "./lib/ipc";
import {
  MessageList,
  type ToolActivity,
  type Turn,
} from "./features/chat/MessageList";
import { ProviderSettings } from "./features/settings/ProviderSettings";
import { WindowControls } from "./WindowControls";
import "./App.css";

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

/**
 * Whether a failed tool result came from the user declining it.
 *
 * The engine reports a denial as an ordinary failure so the model can react;
 * the UI wants to say "declined" rather than "failed".
 */
function isDenial(output: unknown) {
  return (
    typeof output === "object" &&
    output !== null &&
    "error" in output &&
    typeof output.error === "string" &&
    output.error.includes("declined")
  );
}

function toolNames(tools: ToolSpec[]) {
  return tools.map((tool) => tool.name).join(", ");
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
  const [tools, setTools] = useState<ToolSpec[]>([]);
  const [toolsEnabled, setToolsEnabled] = useState(false);

  /** Run id of the in-flight run, needed to answer its approval prompts. */
  const activeRunRef = useRef<string>("");
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSessionId, setActiveSessionId] = useState("");
  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [usage, setUsage] = useState<Usage | null>(null);

  const cancelRef = useRef<(() => void) | null>(null);

  const activeProvider = useMemo(
    () => providers.find((provider) => provider.id === providerId) ?? null,
    [providers, providerId],
  );

  /**
   * Whether tools can actually run right now.
   *
   * ADK does not forward tool calls, and some transports cannot encode them,
   * so the toggle has to reflect both.
   */
  const toolsUsable = Boolean(activeProvider?.supportsTools) && engine === "direct";

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
    listTools()
      .then(setTools)
      .catch((error: unknown) => console.error("failed to load tools", error));
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

  useEffect(() => () => cancelRef.current?.(), []);

  /**
   * Replace local turns with the stored transcript.
   *
   * The store is authoritative: it carries the real ids and `seq` values that
   * regenerate and edit need to truncate from, which optimistic local turns
   * do not have.
   */
  const loadTurns = useCallback(async (sessionId: string) => {
    try {
      const messages = await getMessages(sessionId);
      setTurns(
        messages.map((message) => ({
          id: message.id,
          role: message.role,
          content: message.content,
          seq: message.seq,
        })),
      );
    } catch (error: unknown) {
      console.error("failed to load messages", error);
    }
  }, []);

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
      setTurns([]);
      await loadTurns(session.id);
    },
    [loadTurns],
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

  /**
   * Stream one turn into an existing session.
   *
   * Shared by send, regenerate and edit-and-resend — all three are "append a
   * user turn and stream the reply", differing only in what was truncated
   * first.
   */
  const startRun = useCallback(
    (sessionId: string, prompt: string) => {
      const assistantId = newId();
      setTurns((current) => [
        ...current,
        { id: newId(), role: "user", content: prompt },
        { id: assistantId, role: "assistant", content: "", pending: true },
      ]);
      setIsStreaming(true);
      setUsage(null);

      const settle = (patch: Partial<Turn> = {}) => {
        cancelRef.current = null;
        setIsStreaming(false);
        setTurns((current) =>
          current.map((turn) =>
            turn.id === assistantId
              ? { ...turn, pending: false, ...patch }
              : turn,
          ),
        );
        void refreshSessions();
      };

      /** Add or update one tool card on the streaming assistant turn. */
      const upsertTool = (
        callId: string,
        patch: Partial<ToolActivity> & Pick<ToolActivity, "name" | "args">,
      ) =>
        setTurns((current) =>
          current.map((turn) => {
            if (turn.id !== assistantId) return turn;
            const existing = turn.tools ?? [];
            const index = existing.findIndex(
              (activity) => activity.callId === callId,
            );

            if (index === -1) {
              return {
                ...turn,
                tools: [...existing, { callId, status: "running", ...patch }],
              };
            }

            const updated = [...existing];
            updated[index] = { ...updated[index], ...patch };
            return { ...turn, tools: updated };
          }),
        );

      const runId = crypto.randomUUID();
      activeRunRef.current = runId;

      cancelRef.current = runStream(
        {
          runId,
          sessionId,
          providerId,
          model,
          prompt,
          temperature: 0.7,
          engine,
          toolIds:
            toolsEnabled && toolsUsable ? tools.map((tool) => tool.name) : [],
          // Models will not reach for a tool they were not told they have.
          systemPrompt:
            toolsEnabled && toolsUsable
              ? "You have tools available. Use them whenever the answer depends on live data, the user's files, or anything you cannot know on your own."
              : undefined,
        },
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
            case "tool_call":
              upsertTool(event.id, {
                name: event.name,
                args: event.args,
                status: "running",
              });
              break;
            case "approval_request":
              // The run is blocked here until the user answers.
              upsertTool(event.id, {
                name: event.name,
                args: event.args,
                status: "awaiting",
              });
              break;
            case "tool_result":
              setTurns((current) =>
                current.map((turn) => {
                  if (turn.id !== assistantId) return turn;
                  return {
                    ...turn,
                    tools: (turn.tools ?? []).map((activity) =>
                      activity.callId === event.id
                        ? {
                            ...activity,
                            status: event.ok
                              ? ("ok" as const)
                              : // A refusal reads differently from a crash.
                                isDenial(event.output)
                                ? ("denied" as const)
                                : ("failed" as const),
                            output: event.output,
                          }
                        : activity,
                    ),
                  };
                }),
              );
              break;
            case "done":
              setUsage(event.usage ?? null);
              settle();
              // Reconcile with the store so the new turns pick up their real
              // ids and seq values, which regenerate and edit depend on.
              void loadTurns(sessionId);
              break;
            case "error":
              settle({ content: `⚠ ${event.message}` });
              break;
            default:
              // citation lands with RAG.
              break;
          }
        },
      );
    },
    [
      providerId,
      model,
      engine,
      tools,
      toolsEnabled,
      toolsUsable,
      refreshSessions,
      loadTurns,
    ],
  );

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

    setInput("");
    startRun(sessionId, text);
  }, [
    input,
    isStreaming,
    providerId,
    model,
    engine,
    activeSessionId,
    refreshSessions,
    startRun,
  ]);

  /**
   * Drop the last assistant turn and ask again with the same prompt.
   */
  const regenerate = useCallback(async () => {
    if (isStreaming || !activeSessionId) return;

    const lastAssistant = [...turns]
      .reverse()
      .find((turn) => turn.role === "assistant" && turn.seq !== undefined);
    if (!lastAssistant?.seq) return;

    // The user turn immediately before it is the prompt to replay.
    const prompt = [...turns]
      .reverse()
      .find(
        (turn) =>
          turn.role === "user" &&
          turn.seq !== undefined &&
          turn.seq < lastAssistant.seq!,
      );
    if (!prompt) return;

    try {
      // Truncate from the user turn, since startRun re-appends it.
      await truncateSession(activeSessionId, prompt.seq!);
      setTurns((current) =>
        current.filter((turn) => (turn.seq ?? Infinity) < prompt.seq!),
      );
      startRun(activeSessionId, prompt.content);
    } catch (error: unknown) {
      console.error("regenerate failed", error);
    }
  }, [isStreaming, activeSessionId, turns, startRun]);

  /**
   * Rewrite a user turn and re-run from there, discarding everything after.
   */
  const editAndResend = useCallback(
    async (turn: Turn, next: string) => {
      if (isStreaming || !activeSessionId || turn.seq === undefined) return;

      try {
        await truncateSession(activeSessionId, turn.seq);
        setTurns((current) =>
          current.filter((item) => (item.seq ?? Infinity) < turn.seq!),
        );
        startRun(activeSessionId, next);
      } catch (error: unknown) {
        console.error("edit failed", error);
      }
    },
    [isStreaming, activeSessionId, startRun],
  );

  /** Answer a pending approval prompt for the in-flight run. */
  const respondToPrompt = useCallback((callId: string, approved: boolean) => {
    const runId = activeRunRef.current;
    if (!runId) return;

    // Reflect the decision immediately; the tool_result event will follow and
    // replace this with the real outcome.
    setTurns((current) =>
      current.map((turn) => ({
        ...turn,
        tools: (turn.tools ?? []).map((activity) =>
          activity.callId === callId
            ? { ...activity, status: approved ? "running" : "denied" }
            : activity,
        ),
      })),
    );

    void respondToApproval(runId, callId, approved).catch((error: unknown) =>
      console.error("failed to answer approval", error),
    );
  }, []);

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

            {tools.length > 0 && (
              <label
                className="field field--check"
                title={
                  toolsUsable
                    ? toolNames(tools)
                    : `${activeProvider?.label ?? "This provider"} cannot use tools yet`
                }
              >
                <span className="field__label">Tools</span>
                <span className={`check ${toolsUsable ? "" : "check--off"}`}>
                  <input
                    type="checkbox"
                    checked={toolsEnabled && toolsUsable}
                    // Offering a toggle the backend would discard is worse
                    // than not offering one.
                    disabled={isStreaming || !toolsUsable}
                    onChange={(event) => setToolsEnabled(event.target.checked)}
                  />
                  <span className="check__text">
                    {!toolsUsable
                      ? "unsupported"
                      : toolsEnabled
                        ? `${tools.length} on`
                        : "off"}
                  </span>
                </span>
              </label>
            )}

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

          {turns.length === 0 ? (
            <main className="messages">
              {connection.status === "failed" && (
                <div className="notice notice--error">
                  <strong>
                    Can’t reach {activeProvider?.label ?? "provider"}
                  </strong>
                  <p>{connection.message}</p>
                </div>
              )}

              {connection.status === "ok" && (
                <div className="empty">
                  <h1 className="empty__title">Local and ready</h1>
                  <p className="empty__body">
                    Streaming through {activeProvider?.label}. Nothing leaves
                    this machine.
                  </p>
                </div>
              )}
            </main>
          ) : (
            <MessageList
              turns={turns}
              isBusy={isStreaming}
              onRegenerate={() => void regenerate()}
              onEdit={(turn, next) => void editAndResend(turn, next)}
              onRespond={respondToPrompt}
            />
          )}

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
