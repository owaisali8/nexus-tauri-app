import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  type Agent,
  type EngineEvent,
  type ModelInfo,
  type ProviderView,
  type Session,
  type Usage,
  createSession,
  getMessages,
  listModels,
  respondToApproval,
  runStream,
  truncateSession,
} from "../../lib/ipc";
import { MessageList, type ToolActivity, type Turn } from "./MessageList";

export type ConnectionState =
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

export function ChatView({
  session,
  agent,
  providers,
  onSessionCreated,
  onSessionsChanged,
  onOpenSettings,
}: {
  /** The open conversation, or null for an unsaved new chat. */
  session: Session | null;
  /** The agent this conversation is with, if any. */
  agent: Agent | null;
  providers: ProviderView[];
  onSessionCreated: (session: Session) => void;
  onSessionsChanged: () => void;
  onOpenSettings: () => void;
}) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [usage, setUsage] = useState<Usage | null>(null);
  const [connection, setConnection] = useState<ConnectionState>({
    status: "idle",
  });

  // Plain chat picks its own provider and model; an agent supplies them.
  const [chatProviderId, setChatProviderId] = useState("");
  const [chatModel, setChatModel] = useState("");

  const cancelRef = useRef<(() => void) | null>(null);
  const activeRunRef = useRef<string>("");

  const providerId = agent?.providerId ?? session?.providerId ?? chatProviderId;
  const model = agent?.model ?? session?.model ?? chatModel;

  const activeProvider = useMemo(
    () => providers.find((provider) => provider.id === providerId) ?? null,
    [providers, providerId],
  );

  useEffect(() => {
    if (!chatProviderId && providers[0]) setChatProviderId(providers[0].id);
  }, [providers, chatProviderId]);

  const testConnection = useCallback(async (id: string) => {
    setConnection({ status: "testing" });
    try {
      setConnection({ status: "ok", models: await listModels(id) });
    } catch (error: unknown) {
      setConnection({ status: "failed", message: String(error) });
    }
  }, []);

  useEffect(() => {
    if (providerId) void testConnection(providerId);
  }, [providerId, testConnection]);

  // Pick a default model for plain chat once the provider answers.
  useEffect(() => {
    if (agent || chatModel || connection.status !== "ok") return;
    const chat = connection.models.find(
      (item) => !item.id.toLowerCase().includes("embed"),
    );
    if (chat) setChatModel(chat.id);
  }, [agent, chatModel, connection]);

  /**
   * Replace local turns with the stored transcript.
   *
   * The store is authoritative: it carries the real ids and `seq` values that
   * regenerate and edit need, which optimistic local turns lack.
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

  // Load the transcript once, on mount.
  //
  // Not keyed on `session`: it goes from null to a row on the first send of a
  // new chat, and reacting to that would wipe the messages just streamed in.
  // The parent remounts this component when the user switches conversation,
  // which is the only time a reload is wanted.
  useEffect(() => {
    const openId = session?.id;
    if (openId) void loadTurns(openId);

    // Cancel whatever is in flight when this conversation goes away.
    return () => cancelRef.current?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

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
        onSessionsChanged();
      };

      const runId = newId();
      activeRunRef.current = runId;

      cancelRef.current = runStream(
        {
          runId,
          sessionId,
          providerId,
          model,
          prompt,
          temperature: agent?.temperature ?? 0.7,
          engine: agent?.engine ?? session?.engine ?? "direct",
          systemPrompt: agent?.instructions?.trim() || undefined,
          toolIds: agent?.toolIds ?? [],
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
                              : isDenial(event.output)
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
              void loadTurns(sessionId);
              break;
            case "error":
              settle({ content: `⚠ ${event.message}` });
              break;
            default:
              break;
          }
        },
      );
    },
    [providerId, model, agent, session, onSessionsChanged, loadTurns],
  );

  const send = useCallback(async () => {
    const text = input.trim();
    if (!text || isStreaming || !providerId || !model) return;

    let sessionId = session?.id ?? "";

    // The conversation is created on first send, so abandoned drafts never
    // litter the list.
    if (!sessionId) {
      try {
        const created = await createSession({
          title: deriveTitle(text),
          providerId,
          model,
          engine: agent?.engine ?? "direct",
          agentId: agent?.id ?? null,
        });
        sessionId = created.id;
        onSessionCreated(created);
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
    session,
    agent,
    onSessionCreated,
    startRun,
  ]);

  const stop = useCallback(() => {
    cancelRef.current?.();
    cancelRef.current = null;
    setIsStreaming(false);
    setTurns((current) => current.map((turn) => ({ ...turn, pending: false })));
    onSessionsChanged();
  }, [onSessionsChanged]);

  const regenerate = useCallback(async () => {
    if (isStreaming || !session) return;

    const lastAssistant = [...turns]
      .reverse()
      .find((turn) => turn.role === "assistant" && turn.seq !== undefined);
    if (!lastAssistant?.seq) return;

    const prompt = [...turns]
      .reverse()
      .find(
        (turn) =>
          turn.role === "user" &&
          turn.seq !== undefined &&
          turn.seq < lastAssistant.seq!,
      );
    if (!prompt?.seq && prompt?.seq !== 0) return;

    try {
      await truncateSession(session.id, prompt.seq);
      setTurns((current) =>
        current.filter((turn) => (turn.seq ?? Infinity) < prompt.seq!),
      );
      startRun(session.id, prompt.content);
    } catch (error: unknown) {
      console.error("regenerate failed", error);
    }
  }, [isStreaming, session, turns, startRun]);

  const editAndResend = useCallback(
    async (turn: Turn, next: string) => {
      if (isStreaming || !session || turn.seq === undefined) return;
      try {
        await truncateSession(session.id, turn.seq);
        setTurns((current) =>
          current.filter((item) => (item.seq ?? Infinity) < turn.seq!),
        );
        startRun(session.id, next);
      } catch (error: unknown) {
        console.error("edit failed", error);
      }
    },
    [isStreaming, session, startRun],
  );

  const respondToPrompt = useCallback((callId: string, approved: boolean) => {
    const runId = activeRunRef.current;
    if (!runId) return;

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

  const offline = connection.status === "failed";
  const canSend = Boolean(input.trim() && !isStreaming && providerId && model);

  return (
    <div className="chat">
      <header className="chat__header">
        <div className="chat__identity">
          <span className="chat__title">
            {agent ? agent.name : session?.title || "New chat"}
          </span>
          <span className="chat__subtitle">
            {agent?.description ||
              [activeProvider?.label, model].filter(Boolean).join(" · ") ||
              "No provider configured"}
          </span>
        </div>

        <div className="chat__meta">
          {offline && (
            <button
              type="button"
              className="badge badge--error"
              title={connection.message}
              onClick={onOpenSettings}
            >
              offline
            </button>
          )}
          {agent && agent.toolIds.length > 0 && (
            <span className="badge badge--muted">
              {agent.toolIds.length} tool
              {agent.toolIds.length === 1 ? "" : "s"}
            </span>
          )}
          {usage && (
            <span className="badge badge--muted" title="tokens this turn">
              {usage.totalTokens}
            </span>
          )}
        </div>
      </header>

      {/* Plain chat needs a model picker; an agent already carries one. */}
      {!agent && !session && (
        <div className="chat__pickers">
          <select
            className="field__control"
            value={chatProviderId}
            onChange={(event) => {
              setChatProviderId(event.target.value);
              setChatModel("");
            }}
          >
            {providers.map((provider) => (
              <option key={provider.id} value={provider.id}>
                {provider.label}
              </option>
            ))}
          </select>
          <select
            className="field__control"
            value={chatModel}
            disabled={connection.status !== "ok"}
            onChange={(event) => setChatModel(event.target.value)}
          >
            {connection.status === "ok" ? (
              connection.models
                .filter((item) => !item.id.toLowerCase().includes("embed"))
                .map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.id}
                  </option>
                ))
            ) : (
              <option value="">—</option>
            )}
          </select>
        </div>
      )}

      {turns.length === 0 ? (
        <main className="messages">
          {offline ? (
            <div className="notice notice--error">
              <strong>Can’t reach {activeProvider?.label ?? "provider"}</strong>
              <p>{connection.message}</p>
            </div>
          ) : (
            <div className="empty">
              <h1 className="empty__title">
                {agent ? agent.name : "Ready when you are"}
              </h1>
              <p className="empty__body">
                {agent?.description ||
                  "Ask anything. Nothing leaves this machine unless you pick a cloud provider."}
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
            offline ? "Provider unreachable" : `Message ${agent?.name ?? ""}…`
          }
          disabled={offline}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              void send();
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
            onClick={() => void send()}
          >
            Send
          </button>
        )}
      </footer>
    </div>
  );
}
