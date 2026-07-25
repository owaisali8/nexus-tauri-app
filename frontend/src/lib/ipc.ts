/**
 * Typed wrappers over Tauri commands.
 *
 * These mirror the Rust DTOs in `core`. `EngineEvent` is the single seam
 * between engine and UI — every streamed capability arrives through it.
 */
import { Channel, invoke } from "@tauri-apps/api/core";

export type ProviderKind =
  | "open_ai"
  | "anthropic"
  | "gemini"
  | "deep_seek"
  | "open_ai_compatible";

export type ProviderConfig = {
  id: string;
  label: string;
  kind: ProviderKind;
  baseUrl?: string | null;
  apiKeyRef?: string | null;
  defaultModel?: string | null;
};

/** Provider as returned to the UI — reports secret presence, never its value. */
export type ProviderView = ProviderConfig & {
  hasApiKey: boolean;
  /** Whether this provider's transport forwards tool calls. */
  supportsTools: boolean;
};

export type ModelInfo = { id: string; owned_by?: string | null };

export type ChatMessage = {
  role: "system" | "user" | "assistant";
  content: string;
};

export type Usage = {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
};

export type EngineEvent =
  | { type: "token"; text: string }
  | { type: "tool_call"; id: string; name: string; args: unknown }
  | { type: "approval_request"; id: string; name: string; args: unknown }
  | { type: "tool_result"; id: string; ok: boolean; output: unknown }
  | { type: "citation"; source: string; url?: string | null; snippet: string }
  | { type: "done"; usage?: Usage | null }
  | { type: "error"; message: string };

export type ToolSpec = {
  name: string;
  description: string;
  parameters: unknown;
};

export function listTools(): Promise<ToolSpec[]> {
  return invoke("list_tools");
}

export type McpServerConfig = {
  id: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  enabled: boolean;
};

export type McpServerView = McpServerConfig & {
  connected: boolean;
  /** Tool names this server currently contributes. */
  tools: string[];
  /** Why it failed to start, when it did. */
  error: string | null;
};

export function listMcpServers(): Promise<McpServerView[]> {
  return invoke("list_mcp_servers");
}

/** Save and reconnect. Resolves with the servers that failed to start. */
export function saveMcpServer(server: McpServerConfig): Promise<string[]> {
  return invoke("save_mcp_server", { server });
}

export function deleteMcpServer(serverId: string): Promise<string[]> {
  return invoke("delete_mcp_server", { serverId });
}

export function reconnectMcp(): Promise<string[]> {
  return invoke("reconnect_mcp");
}

export type EmbeddingConfig = {
  providerId: string;
  model: string;
};

export type IngestedDocument = {
  id: string;
  title: string;
  source: string;
  mimeType: string;
  byteCount: number;
  createdAt: number;
  chunkCount: number;
};

export function getEmbeddingConfig(): Promise<EmbeddingConfig | null> {
  return invoke("get_embedding_config");
}

/** Pass `null` to turn document search off. */
export function setEmbeddingConfig(
  config: EmbeddingConfig | null,
): Promise<void> {
  return invoke("set_embedding_config", { config });
}

export function listDocuments(): Promise<IngestedDocument[]> {
  return invoke("list_documents");
}

/** Chunk, embed and index a document. Resolves with the chunk count. */
export function ingestDocument(request: {
  title: string;
  source: string;
  mimeType?: string;
  text: string;
}): Promise<number> {
  return invoke("ingest_document", { request });
}

export function deleteDocument(documentId: string): Promise<boolean> {
  return invoke("delete_document", { documentId });
}

/**
 * Answer a pending approval prompt.
 *
 * Resolves `false` when nothing was waiting — usually the run was cancelled
 * or the prompt timed out.
 */
export function respondToApproval(
  runId: string,
  callId: string,
  approved: boolean,
): Promise<boolean> {
  return invoke("respond_to_approval", { runId, callId, approved });
}

export type Session = {
  id: string;
  title: string;
  providerId: string;
  model: string;
  engine: EngineKind;
  createdAt: number;
  updatedAt: number;
};

/** A persisted transcript entry. `seq` is the ordering key, not `createdAt`. */
export type Message = {
  id: string;
  sessionId: string;
  role: "system" | "user" | "assistant";
  content: string;
  seq: number;
  createdAt: number;
};

export function listSessions(): Promise<Session[]> {
  return invoke("list_sessions");
}

export function createSession(request: {
  title?: string;
  providerId: string;
  model: string;
  engine?: EngineKind;
}): Promise<Session> {
  return invoke("create_session", { request });
}

export function deleteSession(sessionId: string): Promise<void> {
  return invoke("delete_session", { sessionId });
}

export function renameSession(sessionId: string, title: string): Promise<void> {
  return invoke("rename_session", { sessionId, title });
}

export function getMessages(sessionId: string): Promise<Message[]> {
  return invoke("get_messages", { sessionId });
}

/**
 * Drop messages at or after `fromSeq` and clear engine-side caches.
 *
 * Backs regenerate and edit-and-resend. Resolves with the number removed.
 */
export function truncateSession(
  sessionId: string,
  fromSeq: number,
): Promise<number> {
  return invoke("truncate_session", { sessionId, fromSeq });
}

export function listProviders(): Promise<ProviderView[]> {
  return invoke("list_providers");
}

export function saveProvider(
  config: ProviderConfig,
  apiKey?: string,
): Promise<ProviderView> {
  return invoke("save_provider", { request: { ...config, apiKey } });
}

export function deleteProvider(providerId: string): Promise<void> {
  return invoke("delete_provider", { providerId });
}

/** Also serves as the connection test — it throws if the server is unreachable. */
export function listModels(providerId: string): Promise<ModelInfo[]> {
  return invoke("list_models", { providerId });
}

/** Which engine implementation runs the turn. Mirrors core's `EngineKind`. */
export type EngineKind = "direct" | "adk";

export type RunStreamRequest = {
  /**
   * Identifies this run. The caller supplies it because approval prompts are
   * answered by run id, and the UI needs to know it before the run starts.
   */
  runId: string;
  sessionId: string;
  providerId: string;
  model: string;
  /**
   * The new user turn only. Prior turns live in the engine keyed by
   * `sessionId`; the UI must not resend the transcript.
   */
  prompt: string;
  systemPrompt?: string;
  temperature?: number;
  engine?: EngineKind;
  /** Tools this run may use. Omitted or empty means none are offered. */
  toolIds?: string[];
};

/**
 * Start a streamed run. Returns a cancel function; calling it aborts the run
 * server-side and stops further `onEvent` calls.
 *
 * Errors raised before the stream opens (server down, bad config) are
 * delivered as an `error` event rather than a rejected promise, so callers
 * have exactly one error path.
 */
export function runStream(
  request: RunStreamRequest,
  onEvent: (event: EngineEvent) => void,
): () => void {
  const { runId } = request;
  let cancelled = false;

  const channel = new Channel<EngineEvent>();
  channel.onmessage = (event) => {
    if (!cancelled) onEvent(event);
  };

  invoke("run_stream", { request, channel }).catch(
    (error: unknown) => {
      if (!cancelled) {
        onEvent({ type: "error", message: String(error) });
      }
    },
  );

  return () => {
    if (cancelled) return;
    cancelled = true;
    void invoke("cancel_run", { runId });
  };
}
