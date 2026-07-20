import type { ProviderKind } from "../../lib/ipc";

/**
 * Presets for the "add provider" flow.
 *
 * OpenAI and DeepSeek use `open_ai_compatible`: OpenAI's own API *is* that
 * wire format and DeepSeek matches it, so they share the transport local
 * servers already use. Anthropic and Gemini have their own wire formats and
 * their own transports.
 */
export type ProviderTemplate = {
  /** Stable id used for both the config entry and the keychain ref. */
  id: string;
  label: string;
  kind: ProviderKind;
  /** Blank means "use the transport's default endpoint". */
  baseUrl: string;
  requiresApiKey: boolean;
  /** Shown under the form to explain setup or where to get a key. */
  hint: string;
};

export const PROVIDER_TEMPLATES: ProviderTemplate[] = [
  {
    id: "lmstudio-local",
    label: "LM Studio (local)",
    kind: "open_ai_compatible",
    baseUrl: "http://localhost:1234/v1",
    requiresApiKey: false,
    hint: "Start the server from LM Studio's Developer tab, then test the connection.",
  },
  {
    id: "ollama-local",
    label: "Ollama (local)",
    kind: "open_ai_compatible",
    baseUrl: "http://localhost:11434/v1",
    requiresApiKey: false,
    hint: "Run `ollama serve`, then test the connection.",
  },
  {
    id: "openai",
    label: "OpenAI",
    kind: "open_ai_compatible",
    baseUrl: "https://api.openai.com/v1",
    requiresApiKey: true,
    hint: "The key is stored in your OS keychain, never on disk or in this app's config.",
  },
  {
    id: "anthropic",
    label: "Anthropic",
    kind: "anthropic",
    baseUrl: "https://api.anthropic.com",
    requiresApiKey: true,
    hint: "Uses the Messages API. The key is stored in your OS keychain, never on disk or in this app's config.",
  },
  {
    id: "gemini",
    label: "Google Gemini",
    kind: "gemini",
    baseUrl: "https://generativelanguage.googleapis.com",
    requiresApiKey: true,
    hint: "Uses the Gemini API. The key is sent as a header, never in the URL, and is stored in your OS keychain.",
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    kind: "open_ai_compatible",
    baseUrl: "https://api.deepseek.com/v1",
    requiresApiKey: true,
    hint: "The key is stored in your OS keychain, never on disk or in this app's config.",
  },
  {
    id: "custom",
    label: "Custom server",
    kind: "open_ai_compatible",
    baseUrl: "",
    requiresApiKey: false,
    hint: "Any OpenAI-compatible endpoint — vLLM, LiteLLM, a proxy. Include the /v1 suffix.",
  },
];

export function templateById(id: string): ProviderTemplate | undefined {
  return PROVIDER_TEMPLATES.find((template) => template.id === id);
}

/** Turn a label into a URL-safe id. */
function slugify(value: string) {
  return (
    value
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 40) || "provider"
  );
}

/**
 * Pick an id that no existing provider is using.
 *
 * Two custom servers with the same label would otherwise collide and the
 * second would silently overwrite the first, taking its keychain entry with it.
 */
export function uniqueProviderId(preferred: string, taken: string[]): string {
  const base = slugify(preferred);
  if (!taken.includes(base)) return base;

  for (let suffix = 2; suffix < 100; suffix += 1) {
    const candidate = `${base}-${suffix}`;
    if (!taken.includes(candidate)) return candidate;
  }
  return `${base}-${Date.now()}`;
}
