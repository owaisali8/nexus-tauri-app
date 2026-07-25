import { useCallback, useEffect, useState } from "react";
import {
  type Agent,
  type EngineKind,
  type ProviderView,
  type ToolSpec,
  deleteAgent,
  listModels,
  saveAgent,
} from "../../lib/ipc";

/** A new agent, before it has been saved. */
function blankAgent(providerId: string): Agent {
  return {
    id: "",
    name: "",
    description: "",
    instructions: "",
    providerId,
    model: "",
    temperature: 0.7,
    toolIds: [],
    engine: "direct",
    createdAt: 0,
    updatedAt: 0,
  };
}

/** Starting points, so the first agent is not a blank form. */
const TEMPLATES: { label: string; description: string; instructions: string }[] = [
  {
    label: "Research assistant",
    description: "Answers from your indexed documents",
    instructions:
      "You answer questions using the user's own documents. Search them before " +
      "answering, cite the source of each claim, and say plainly when the " +
      "documents do not cover something rather than filling the gap from " +
      "general knowledge.",
  },
  {
    label: "Code reviewer",
    description: "Reviews diffs and explains trade-offs",
    instructions:
      "You review code. Point out correctness problems first, then design " +
      "issues, then style. Be specific about why something matters and what " +
      "would go wrong. Skip praise unless it is load-bearing.",
  },
  {
    label: "Writing editor",
    description: "Tightens prose without changing the voice",
    instructions:
      "You edit prose. Cut redundancy, prefer concrete words, and keep the " +
      "author's voice. Explain each substantive change in one line so the " +
      "author can disagree.",
  },
];

export function AgentsView({
  agents,
  providers,
  tools,
  onChanged,
  onStartChat,
}: {
  agents: Agent[];
  providers: ProviderView[];
  tools: ToolSpec[];
  onChanged: () => void;
  onStartChat: (agent: Agent) => void;
}) {
  const [draft, setDraft] = useState<Agent | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  // Only chat models: an embedding model here produces an error at run time,
  // long after the mistake was made.
  useEffect(() => {
    if (!draft?.providerId) return;
    let cancelled = false;

    void (async () => {
      try {
        const available = await listModels(draft.providerId);
        if (cancelled) return;
        const chat = available
          .map((model) => model.id)
          .filter((id) => !id.toLowerCase().includes("embed"));
        setModels(chat);
        setDraft((current) =>
          current && !current.model
            ? { ...current, model: chat[0] ?? "" }
            : current,
        );
      } catch {
        if (!cancelled) setModels([]);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [draft?.providerId]);

  const startNew = useCallback(
    (template?: (typeof TEMPLATES)[number]) => {
      setError("");
      const base = blankAgent(providers[0]?.id ?? "");
      setDraft(
        template
          ? {
              ...base,
              name: template.label,
              description: template.description,
              instructions: template.instructions,
            }
          : base,
      );
    },
    [providers],
  );

  const save = async () => {
    if (!draft) return;
    if (!draft.name.trim()) {
      setError("Give the agent a name.");
      return;
    }
    if (!draft.providerId || !draft.model) {
      setError("Pick a provider and a model.");
      return;
    }

    setBusy(true);
    try {
      await saveAgent(draft);
      setDraft(null);
      onChanged();
      setError("");
    } catch (caught: unknown) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (agent: Agent) => {
    if (
      !window.confirm(
        `Delete ${agent.name}? Conversations you had with it are kept.`,
      )
    ) {
      return;
    }
    try {
      await deleteAgent(agent.id);
      onChanged();
    } catch (caught: unknown) {
      setError(String(caught));
    }
  };

  const toggleTool = (name: string) => {
    if (!draft) return;
    setDraft({
      ...draft,
      toolIds: draft.toolIds.includes(name)
        ? draft.toolIds.filter((id) => id !== name)
        : [...draft.toolIds, name],
    });
  };

  if (draft) {
    return (
      <div className="view">
        <header className="view__header">
          <h1 className="view__title">
            {draft.id ? "Edit agent" : "New agent"}
          </h1>
          <div className="view__actions">
            <button
              type="button"
              className="button button--ghost"
              onClick={() => {
                setDraft(null);
                setError("");
              }}
            >
              Cancel
            </button>
            <button
              type="button"
              className="button button--send"
              disabled={busy}
              onClick={() => void save()}
            >
              {busy ? "Saving…" : "Save agent"}
            </button>
          </div>
        </header>

        <div className="view__body">
          {error && <div className="notice notice--error">{error}</div>}

          <div className="form-grid">
            <label className="field field--block">
              <span className="field__label">Name</span>
              <input
                className="field__control field__control--wide"
                value={draft.name}
                placeholder="Research assistant"
                onChange={(event) =>
                  setDraft({ ...draft, name: event.target.value })
                }
              />
            </label>

            <label className="field field--block">
              <span className="field__label">Description</span>
              <input
                className="field__control field__control--wide"
                value={draft.description}
                placeholder="What this agent is for"
                onChange={(event) =>
                  setDraft({ ...draft, description: event.target.value })
                }
              />
            </label>
          </div>

          <label className="field field--block">
            <span className="field__label">Instructions</span>
            <textarea
              className="field__control field__control--wide instructions"
              value={draft.instructions}
              rows={8}
              placeholder="How this agent should behave. Sent as the system prompt."
              onChange={(event) =>
                setDraft({ ...draft, instructions: event.target.value })
              }
            />
          </label>

          <div className="form-grid">
            <label className="field field--block">
              <span className="field__label">Provider</span>
              <select
                className="field__control field__control--wide"
                value={draft.providerId}
                onChange={(event) =>
                  setDraft({
                    ...draft,
                    providerId: event.target.value,
                    model: "",
                  })
                }
              >
                {providers.map((provider) => (
                  <option key={provider.id} value={provider.id}>
                    {provider.label}
                  </option>
                ))}
              </select>
            </label>

            <label className="field field--block">
              <span className="field__label">Model</span>
              <select
                className="field__control field__control--wide"
                value={draft.model}
                disabled={models.length === 0}
                onChange={(event) =>
                  setDraft({ ...draft, model: event.target.value })
                }
              >
                {models.length === 0 ? (
                  <option value="">provider unreachable</option>
                ) : (
                  models.map((model) => (
                    <option key={model} value={model}>
                      {model}
                    </option>
                  ))
                )}
              </select>
            </label>

            <label className="field field--block">
              <span className="field__label">
                Temperature {draft.temperature?.toFixed(1) ?? "—"}
              </span>
              <input
                type="range"
                min={0}
                max={2}
                step={0.1}
                value={draft.temperature ?? 0.7}
                onChange={(event) =>
                  setDraft({
                    ...draft,
                    temperature: Number(event.target.value),
                  })
                }
              />
            </label>
          </div>

          <section className="field field--block">
            <span className="field__label">Tools</span>
            <p className="provider-form__hint">
              Only the tools you tick are offered to this agent. Side-effecting
              ones still ask before they run.
            </p>
            {tools.length === 0 ? (
              <p className="sidebar__empty">
                No tools available. Connect an MCP server or enable document
                search.
              </p>
            ) : (
              <div className="tool-picker">
                {tools.map((tool) => (
                  <label key={tool.name} className="tool-pick">
                    <input
                      type="checkbox"
                      checked={draft.toolIds.includes(tool.name)}
                      onChange={() => toggleTool(tool.name)}
                    />
                    <span>
                      <span className="tool-pick__name">{tool.name}</span>
                      <span className="tool-pick__desc">
                        {tool.description.slice(0, 90)}
                      </span>
                    </span>
                  </label>
                ))}
              </div>
            )}
          </section>
        </div>
      </div>
    );
  }

  return (
    <div className="view">
      <header className="view__header">
        <h1 className="view__title">Agents</h1>
        <div className="view__actions">
          <button
            type="button"
            className="button button--send"
            onClick={() => startNew()}
          >
            New agent
          </button>
        </div>
      </header>

      <div className="view__body">
        {error && <div className="notice notice--error">{error}</div>}

        {agents.length === 0 && (
          <section className="empty-state">
            <h2 className="empty__title">No agents yet</h2>
            <p className="empty__body">
              An agent bundles instructions, a model and a set of tools so you
              do not configure them every time. Start from a template:
            </p>
            <div className="template-grid">
              {TEMPLATES.map((template) => (
                <button
                  key={template.label}
                  type="button"
                  className="template"
                  onClick={() => startNew(template)}
                >
                  <span className="template__name">{template.label}</span>
                  <span className="template__desc">{template.description}</span>
                </button>
              ))}
            </div>
          </section>
        )}

        <div className="agent-grid">
          {agents.map((agent) => (
            <article key={agent.id} className="agent-card">
              <header className="agent-card__head">
                <span className="agent-card__name">{agent.name}</span>
                <span className="agent-card__model">{agent.model}</span>
              </header>

              {agent.description && (
                <p className="agent-card__desc">{agent.description}</p>
              )}

              {agent.toolIds.length > 0 && (
                <p className="agent-card__tools">
                  {agent.toolIds.length} tool
                  {agent.toolIds.length === 1 ? "" : "s"}
                </p>
              )}

              <footer className="agent-card__actions">
                <button
                  type="button"
                  className="button button--send"
                  onClick={() => onStartChat(agent)}
                >
                  Chat
                </button>
                <button
                  type="button"
                  className="button button--ghost"
                  onClick={() => setDraft(agent)}
                >
                  Edit
                </button>
                <button
                  type="button"
                  className="button button--ghost button--danger"
                  onClick={() => void remove(agent)}
                >
                  Delete
                </button>
              </footer>
            </article>
          ))}
        </div>
      </div>
    </div>
  );
}

export type { EngineKind };
