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
export type ProviderView = ProviderConfig & { hasApiKey: boolean };

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
  | { type: "tool_result"; id: string; ok: boolean; output: unknown }
  | { type: "citation"; source: string; url?: string | null; snippet: string }
  | { type: "done"; usage?: Usage | null }
  | { type: "error"; message: string };

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
  const runId = crypto.randomUUID();
  let cancelled = false;

  const channel = new Channel<EngineEvent>();
  channel.onmessage = (event) => {
    if (!cancelled) onEvent(event);
  };

  invoke("run_stream", { request: { runId, ...request }, channel }).catch(
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
