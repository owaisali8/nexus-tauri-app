import { useCallback, useEffect, useRef, useState } from "react";
import {
  type EmbeddingConfig,
  type IngestedDocument,
  type ProviderView,
  deleteDocument,
  getEmbeddingConfig,
  ingestDocument,
  listDocuments,
  listModels,
  listProviders,
  setEmbeddingConfig,
} from "../../lib/ipc";

/**
 * Extensions read as text.
 *
 * Binary formats (PDF, docx) need a parser that does not exist yet; accepting
 * them would index mojibake that then matches nothing.
 */
const TEXT_EXTENSIONS = [
  ".txt", ".md", ".markdown", ".rst", ".org",
  ".json", ".yaml", ".yml", ".toml", ".xml", ".csv",
  ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java",
  ".c", ".h", ".cpp", ".hpp", ".cs", ".rb", ".php", ".sh", ".sql",
  ".html", ".css", ".log",
];

function isTextFile(name: string) {
  const lower = name.toLowerCase();
  return TEXT_EXTENSIONS.some((extension) => lower.endsWith(extension));
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export function DocumentSettings({
  onClose,
  onChanged,
}: {
  onClose: () => void;
  onChanged: () => void;
}) {
  const [documents, setDocuments] = useState<IngestedDocument[]>([]);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [config, setConfig] = useState<EmbeddingConfig | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [draftProvider, setDraftProvider] = useState("");
  const [draftModel, setDraftModel] = useState("");

  const [error, setError] = useState("");
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState(false);

  const fileInput = useRef<HTMLInputElement | null>(null);

  const refresh = useCallback(async () => {
    try {
      setDocuments(await listDocuments());
    } catch (caught: unknown) {
      setError(String(caught));
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [loadedDocuments, loadedProviders, loadedConfig] = await Promise.all([
          listDocuments(),
          listProviders(),
          getEmbeddingConfig(),
        ]);
        if (cancelled) return;

        setDocuments(loadedDocuments);
        setProviders(loadedProviders);
        setConfig(loadedConfig);
        setDraftProvider(loadedConfig?.providerId ?? loadedProviders[0]?.id ?? "");
        setDraftModel(loadedConfig?.model ?? "");
      } catch (caught: unknown) {
        if (!cancelled) setError(String(caught));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Offer only embedding models: a chat model at this endpoint returns an
  // error, or worse, something that is not an embedding.
  useEffect(() => {
    if (!draftProvider) return;
    let cancelled = false;

    void (async () => {
      try {
        const available = await listModels(draftProvider);
        if (cancelled) return;
        const embedding = available
          .map((model) => model.id)
          .filter((id) => id.toLowerCase().includes("embed"));
        setModels(embedding);
        setDraftModel((current) => current || embedding[0] || "");
      } catch {
        if (!cancelled) setModels([]);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [draftProvider]);

  const applyConfig = async () => {
    if (!draftProvider || !draftModel) {
      setError("Pick a provider and an embedding model.");
      return;
    }

    setBusy(true);
    setError("");
    try {
      await setEmbeddingConfig({ providerId: draftProvider, model: draftModel });
      setConfig({ providerId: draftProvider, model: draftModel });
      setStatus("Embedding model set. Documents indexed with a different model need re-adding.");
      onChanged();
    } catch (caught: unknown) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const ingestFiles = async (files: FileList | null) => {
    if (!files || files.length === 0) return;

    if (!config) {
      setError("Choose an embedding model before adding documents.");
      return;
    }

    setBusy(true);
    setError("");

    const skipped: string[] = [];
    let added = 0;

    for (const file of Array.from(files)) {
      if (!isTextFile(file.name)) {
        skipped.push(file.name);
        continue;
      }

      try {
        const text = await file.text();
        const chunks = await ingestDocument({
          title: file.name,
          // The browser exposes no real path, so the name is the identity —
          // re-adding a file with the same name replaces it.
          source: file.name,
          mimeType: file.type || "text/plain",
          text,
        });
        added += chunks;
      } catch (caught: unknown) {
        setError(`${file.name}: ${String(caught)}`);
      }
    }

    setStatus(
      [
        added > 0 ? `Indexed ${added} chunk${added === 1 ? "" : "s"}.` : "",
        skipped.length > 0 ? `Skipped ${skipped.join(", ")} — text files only.` : "",
      ]
        .filter(Boolean)
        .join(" "),
    );

    await refresh();
    onChanged();
    setBusy(false);
  };

  const remove = async (document: IngestedDocument) => {
    if (!window.confirm(`Remove ${document.title} from the index?`)) return;
    try {
      await deleteDocument(document.id);
      await refresh();
      onChanged();
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
      <div className="modal" role="dialog" aria-modal="true" aria-label="Documents">
        <header className="modal__header">
          <h2 className="modal__title">Documents</h2>
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
          {status && !error && <div className="notice notice--ok">{status}</div>}

          <section className="provider-form">
            <span className="field__label">Embedding model</span>
            <p className="provider-form__hint">
              Indexed text is compared using this model. Changing it makes
              existing documents unsearchable until they are added again, since
              vectors from different models are not comparable.
            </p>

            <div className="embed-config">
              <select
                className="field__control"
                value={draftProvider}
                onChange={(event) => {
                  setDraftProvider(event.target.value);
                  setDraftModel("");
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
                value={draftModel}
                disabled={models.length === 0}
                onChange={(event) => setDraftModel(event.target.value)}
              >
                {models.length === 0 ? (
                  <option value="">no embedding model found</option>
                ) : (
                  models.map((model) => (
                    <option key={model} value={model}>
                      {model}
                    </option>
                  ))
                )}
              </select>

              <button
                type="button"
                className="button button--send"
                disabled={busy || !draftModel}
                onClick={() => void applyConfig()}
              >
                {config ? "Update" : "Enable"}
              </button>
            </div>
          </section>

          <section className="provider-add">
            <span className="field__label">Add documents</span>
            <input
              ref={fileInput}
              type="file"
              multiple
              hidden
              onChange={(event) => {
                void ingestFiles(event.target.files);
                // Clear, so re-selecting the same file fires again.
                event.target.value = "";
              }}
            />
            <div
              className="dropzone"
              onDragOver={(event) => event.preventDefault()}
              onDrop={(event) => {
                event.preventDefault();
                void ingestFiles(event.dataTransfer.files);
              }}
              onClick={() => fileInput.current?.click()}
            >
              {busy
                ? "Indexing…"
                : config
                  ? "Drop text files here, or click to choose"
                  : "Choose an embedding model first"}
            </div>
          </section>

          <section className="provider-list">
            {documents.length === 0 && (
              <p className="sidebar__empty">Nothing indexed yet.</p>
            )}
            {documents.map((document) => (
              <div key={document.id} className="provider-row">
                <div className="provider-row__main">
                  <span className="provider-row__label">{document.title}</span>
                  <span className="provider-row__url">
                    {document.chunkCount} chunk
                    {document.chunkCount === 1 ? "" : "s"} ·{" "}
                    {formatBytes(document.byteCount)}
                  </span>
                </div>
                <button
                  type="button"
                  className="button button--ghost button--danger"
                  onClick={() => void remove(document)}
                >
                  Remove
                </button>
              </div>
            ))}
          </section>
        </div>
      </div>
    </div>
  );
}
