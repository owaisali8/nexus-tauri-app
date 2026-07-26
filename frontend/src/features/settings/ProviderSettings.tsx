import { useCallback, useEffect, useState } from "react";
import {
  type ProviderView,
  deleteProvider,
  listModels,
  listProviders,
  saveProvider,
} from "../../lib/ipc";
import {
  PROVIDER_TEMPLATES,
  type ProviderTemplate,
  templateById,
  uniqueProviderId,
} from "./providerTemplates";

type Draft = {
  id: string;
  label: string;
  baseUrl: string;
  apiKey: string;
  defaultModel: string;
  templateId: string;
  /** True when editing an existing entry, so the id is fixed. */
  isExisting: boolean;
  /** Whether a secret is already in the keychain for this provider. */
  hasStoredKey: boolean;
};

type TestState =
  | { status: "idle" }
  | { status: "testing" }
  | { status: "ok"; models: string[] }
  | { status: "failed"; message: string };

function draftFromTemplate(template: ProviderTemplate, taken: string[]): Draft {
  return {
    id: uniqueProviderId(template.id === "custom" ? "custom" : template.id, taken),
    label: template.label,
    baseUrl: template.baseUrl,
    apiKey: "",
    defaultModel: "",
    templateId: template.id,
    isExisting: false,
    hasStoredKey: false,
  };
}

function draftFromProvider(provider: ProviderView): Draft {
  return {
    id: provider.id,
    label: provider.label,
    baseUrl: provider.baseUrl ?? "",
    apiKey: "",
    defaultModel: provider.defaultModel ?? "",
    // Match on id so an edited entry keeps its original hint.
    templateId: templateById(provider.id) ? provider.id : "custom",
    isExisting: true,
    hasStoredKey: provider.hasApiKey,
  };
}

export function ProviderSettings({
  onClose,
  onChanged,
}: {
  onClose: () => void;
  onChanged: () => void;
}) {
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [test, setTest] = useState<TestState>({ status: "idle" });
  const [error, setError] = useState("");
  const [isSaving, setIsSaving] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setProviders(await listProviders());
    } catch (caught: unknown) {
      setError(String(caught));
    }
  }, []);

  // Initial load. Guarded so a dialog closed mid-request does not set state
  // on an unmounted component.
  useEffect(() => {
    let cancelled = false;

    void (async () => {
      try {
        const loaded = await listProviders();
        if (!cancelled) setProviders(loaded);
      } catch (caught: unknown) {
        if (!cancelled) setError(String(caught));
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  // Escape closes the dialog, matching the rest of the app's keyboard-first feel.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const template = draft ? templateById(draft.templateId) : undefined;
  const needsKey = Boolean(template?.requiresApiKey);

  const startAdd = (templateId: string) => {
    const chosen = templateById(templateId);
    if (!chosen) return;
    setError("");
    setTest({ status: "idle" });
    setDraft(
      draftFromTemplate(
        chosen,
        providers.map((provider) => provider.id),
      ),
    );
  };

  const startEdit = (provider: ProviderView) => {
    setError("");
    setTest({ status: "idle" });
    setDraft(draftFromProvider(provider));
  };

  /**
   * Test against the draft's current values rather than what is saved, so a
   * typo in the URL surfaces before it is written to disk. That requires
   * saving first — the backend resolves the key from the keychain by id — so
   * this doubles as "save and verify".
   */
  const runTest = async () => {
    if (!draft) return;
    setTest({ status: "testing" });
    try {
      await persist(draft);
      const models = await listModels(draft.id);
      setTest({ status: "ok", models: models.map((model) => model.id) });
      await refresh();
      onChanged();
    } catch (caught: unknown) {
      setTest({ status: "failed", message: String(caught) });
    }
  };

  const persist = async (current: Draft) => {
    await saveProvider(
      {
        id: current.id,
        label: current.label.trim() || current.id,
        kind: template?.kind ?? "open_ai_compatible",
        baseUrl: current.baseUrl.trim(),
        // Sending undefined leaves any stored secret untouched, which is what
        // an empty field should mean when editing.
        defaultModel: current.defaultModel.trim() || null,
      },
      current.apiKey.trim() || undefined,
    );
  };

  const save = async () => {
    if (!draft) return;
    setError("");

    if (!draft.baseUrl.trim()) {
      setError("A base URL is required. Include the /v1 suffix.");
      return;
    }
    if (needsKey && !draft.apiKey.trim() && !draft.hasStoredKey) {
      setError(`${draft.label} requires an API key.`);
      return;
    }

    setIsSaving(true);
    try {
      await persist(draft);
      await refresh();
      onChanged();
      setDraft(null);
      setTest({ status: "idle" });
    } catch (caught: unknown) {
      setError(String(caught));
    } finally {
      setIsSaving(false);
    }
  };

  const remove = async (provider: ProviderView) => {
    if (
      !window.confirm(
        `Remove ${provider.label}? Its saved API key will be deleted from the keychain.`,
      )
    ) {
      return;
    }
    try {
      await deleteProvider(provider.id);
      await refresh();
      onChanged();
      if (draft?.id === provider.id) setDraft(null);
    } catch (caught: unknown) {
      setError(String(caught));
    }
  };

  return (
    <div
      className="modal__backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label="Provider settings"
      >
        <header className="modal__header">
          <h2 className="modal__title">Providers</h2>
          <button
            type="button"
            className="modal__close"
            aria-label="Close"
            onClick={onClose}
          >
            ×
          </button>
        </header>

        <div className="modal__body">
          {error && <div className="notice notice--error">{error}</div>}

          <section className="provider-list">
            {providers.length === 0 && (
              <p className="sidebar__empty">
                No providers configured. Add one below.
              </p>
            )}
            {providers.map((provider) => (
              <div key={provider.id} className="provider-row">
                <div className="provider-row__main">
                  <span className="provider-row__label">{provider.label}</span>
                  <span className="provider-row__url">
                    {provider.baseUrl ?? "—"}
                  </span>
                </div>
                <span
                  className={`badge ${
                    provider.hasApiKey ? "badge--ok" : "badge--muted"
                  }`}
                  title={
                    provider.hasApiKey
                      ? "An API key is stored in the OS keychain"
                      : "No API key stored"
                  }
                >
                  {provider.hasApiKey ? "key set" : "no key"}
                </span>
                <button
                  type="button"
                  className="button button--ghost"
                  onClick={() => startEdit(provider)}
                >
                  Edit
                </button>
                <button
                  type="button"
                  className="button button--ghost button--danger"
                  onClick={() => void remove(provider)}
                >
                  Remove
                </button>
              </div>
            ))}
          </section>

          {!draft && (
            <section className="provider-add">
              <span className="field__label">Add a provider</span>
              <div className="provider-add__options">
                {PROVIDER_TEMPLATES.map((item) => (
                  <button
                    key={item.id}
                    type="button"
                    className="button button--ghost"
                    onClick={() => startAdd(item.id)}
                  >
                    {item.label}
                  </button>
                ))}
              </div>
            </section>
          )}

          {draft && (
            <section className="provider-form">
              <label className="field field--block">
                <span className="field__label">Name</span>
                <input
                  className="field__control field__control--wide"
                  value={draft.label}
                  onChange={(event) =>
                    setDraft({ ...draft, label: event.target.value })
                  }
                />
              </label>

              <label className="field field--block">
                <span className="field__label">Base URL</span>
                <input
                  className="field__control field__control--wide"
                  value={draft.baseUrl}
                  placeholder="http://localhost:1234/v1"
                  spellCheck={false}
                  onChange={(event) =>
                    setDraft({ ...draft, baseUrl: event.target.value })
                  }
                />
              </label>

              <label className="field field--block">
                <span className="field__label">
                  API key{needsKey ? "" : " (optional)"}
                </span>
                <input
                  className="field__control field__control--wide"
                  type="password"
                  value={draft.apiKey}
                  autoComplete="off"
                  placeholder={
                    draft.hasStoredKey
                      ? "•••••••• stored — leave blank to keep"
                      : "Stored in your OS keychain"
                  }
                  onChange={(event) =>
                    setDraft({ ...draft, apiKey: event.target.value })
                  }
                />
              </label>

              {template?.hint && <p className="provider-form__hint">{template.hint}</p>}

              {test.status === "ok" && (
                <div className="notice notice--ok">
                  Connected — {test.models.length} model
                  {test.models.length === 1 ? "" : "s"} available
                  {test.models.length > 0 && `: ${test.models.join(", ")}`}
                </div>
              )}
              {test.status === "failed" && (
                <div className="notice notice--error">{test.message}</div>
              )}

              <div className="provider-form__actions">
                <button
                  type="button"
                  className="button button--ghost"
                  disabled={test.status === "testing" || !draft.baseUrl.trim()}
                  onClick={() => void runTest()}
                >
                  {test.status === "testing" ? "Testing…" : "Save & test"}
                </button>
                <div className="provider-form__spacer" />
                <button
                  type="button"
                  className="button button--ghost"
                  onClick={() => {
                    setDraft(null);
                    setTest({ status: "idle" });
                    setError("");
                  }}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="button button--send"
                  disabled={isSaving}
                  onClick={() => void save()}
                >
                  {isSaving ? "Saving…" : "Save"}
                </button>
              </div>
            </section>
          )}
        </div>
      </div>
    </div>
  );
}
