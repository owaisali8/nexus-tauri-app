import { useCallback, useEffect, useState } from "react";
import {
  type McpServerView,
  deleteMcpServer,
  listMcpServers,
  reconnectMcp,
  saveMcpServer,
} from "../../lib/ipc";

type Draft = {
  id: string;
  command: string;
  /** Space-separated for editing; split on save. */
  args: string;
  enabled: boolean;
  isExisting: boolean;
};

const BLANK: Draft = {
  id: "",
  command: "",
  args: "",
  enabled: true,
  isExisting: false,
};

/** Presets for servers people commonly run. */
const EXAMPLES = [
  {
    label: "Filesystem",
    id: "filesystem",
    command: "npx",
    args: "-y @modelcontextprotocol/server-filesystem .",
  },
  {
    label: "Memory",
    id: "memory",
    command: "npx",
    args: "-y @modelcontextprotocol/server-memory",
  },
];

/**
 * Split a command line into arguments.
 *
 * Handles quoted segments so a path containing spaces survives, which the
 * filesystem server needs for a Windows root.
 */
function splitArgs(input: string): string[] {
  const out: string[] = [];
  const pattern = /"([^"]*)"|'([^']*)'|(\S+)/g;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(input)) !== null) {
    out.push(match[1] ?? match[2] ?? match[3] ?? "");
  }
  return out.filter((arg) => arg.length > 0);
}

export function McpSettings({
  onClose,
  onChanged,
}: {
  onClose: () => void;
  onChanged: () => void;
}) {
  const [servers, setServers] = useState<McpServerView[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setServers(await listMcpServers());
    } catch (caught: unknown) {
      setError(String(caught));
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const loaded = await listMcpServers();
        if (!cancelled) setServers(loaded);
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

  /** Report which servers failed, since a save can partly succeed. */
  const applyFailures = (failures: string[]) => {
    setError(
      failures.length > 0
        ? `Some servers did not start:\n${failures.join("\n")}`
        : "",
    );
  };

  const save = async () => {
    if (!draft) return;
    if (!draft.id.trim() || !draft.command.trim()) {
      setError("An id and a command are both required.");
      return;
    }

    setBusy(true);
    try {
      const failures = await saveMcpServer({
        id: draft.id.trim(),
        command: draft.command.trim(),
        args: splitArgs(draft.args),
        env: {},
        enabled: draft.enabled,
      });
      applyFailures(failures);
      await refresh();
      onChanged();
      setDraft(null);
    } catch (caught: unknown) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (server: McpServerView) => {
    if (!window.confirm(`Remove the ${server.id} server?`)) return;

    setBusy(true);
    try {
      applyFailures(await deleteMcpServer(server.id));
      await refresh();
      onChanged();
    } catch (caught: unknown) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const toggle = async (server: McpServerView) => {
    setBusy(true);
    try {
      // Send only the config fields; connected/tools/error are read-only view
      // state the backend does not accept.
      applyFailures(
        await saveMcpServer({
          id: server.id,
          command: server.command,
          args: server.args,
          env: server.env,
          enabled: !server.enabled,
        }),
      );
      await refresh();
      onChanged();
    } catch (caught: unknown) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  const reconnect = async () => {
    setBusy(true);
    try {
      applyFailures(await reconnectMcp());
      await refresh();
      onChanged();
    } catch (caught: unknown) {
      setError(String(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="modal__backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="modal" role="dialog" aria-modal="true" aria-label="MCP servers">
        <header className="modal__header">
          <h2 className="modal__title">MCP servers</h2>
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

          <p className="provider-form__hint">
            MCP servers run as programs on your machine. Every tool they expose
            asks for your approval before it runs, because a server describes
            its own tools and that description is not something to trust.
          </p>

          <section className="provider-list">
            {servers.length === 0 && (
              <p className="sidebar__empty">No servers configured.</p>
            )}
            {servers.map((server) => (
              <div key={server.id} className="provider-row">
                <div className="provider-row__main">
                  <span className="provider-row__label">{server.id}</span>
                  <span className="provider-row__url">
                    {server.command} {server.args.join(" ")}
                  </span>
                  {server.error && (
                    <span className="mcp-row__error">{server.error}</span>
                  )}
                </div>

                <span
                  className={`badge ${
                    server.connected
                      ? "badge--ok"
                      : server.enabled
                        ? "badge--error"
                        : "badge--muted"
                  }`}
                  title={
                    server.connected
                      ? server.tools.join(", ")
                      : server.enabled
                        ? "Not connected"
                        : "Disabled"
                  }
                >
                  {server.connected
                    ? `${server.tools.length} tool${server.tools.length === 1 ? "" : "s"}`
                    : server.enabled
                      ? "offline"
                      : "disabled"}
                </span>

                <button
                  type="button"
                  className="button button--ghost"
                  disabled={busy}
                  onClick={() => void toggle(server)}
                >
                  {server.enabled ? "Disable" : "Enable"}
                </button>
                <button
                  type="button"
                  className="button button--ghost button--danger"
                  disabled={busy}
                  onClick={() => void remove(server)}
                >
                  Remove
                </button>
              </div>
            ))}
          </section>

          {!draft && (
            <section className="provider-add">
              <span className="field__label">Add a server</span>
              <div className="provider-add__options">
                {EXAMPLES.map((example) => (
                  <button
                    key={example.id}
                    type="button"
                    className="button button--ghost"
                    onClick={() =>
                      setDraft({
                        id: example.id,
                        command: example.command,
                        args: example.args,
                        enabled: true,
                        isExisting: false,
                      })
                    }
                  >
                    {example.label}
                  </button>
                ))}
                <button
                  type="button"
                  className="button button--ghost"
                  onClick={() => setDraft({ ...BLANK })}
                >
                  Custom
                </button>
                <div className="provider-form__spacer" />
                <button
                  type="button"
                  className="button button--ghost"
                  disabled={busy || servers.length === 0}
                  onClick={() => void reconnect()}
                >
                  {busy ? "Working…" : "Reconnect all"}
                </button>
              </div>
            </section>
          )}

          {draft && (
            <section className="provider-form">
              <label className="field field--block">
                <span className="field__label">Id</span>
                <input
                  className="field__control field__control--wide"
                  value={draft.id}
                  placeholder="filesystem"
                  spellCheck={false}
                  onChange={(event) =>
                    setDraft({ ...draft, id: event.target.value })
                  }
                />
              </label>

              <label className="field field--block">
                <span className="field__label">Command</span>
                <input
                  className="field__control field__control--wide"
                  value={draft.command}
                  placeholder="npx"
                  spellCheck={false}
                  onChange={(event) =>
                    setDraft({ ...draft, command: event.target.value })
                  }
                />
              </label>

              <label className="field field--block">
                <span className="field__label">Arguments</span>
                <input
                  className="field__control field__control--wide"
                  value={draft.args}
                  placeholder="-y @modelcontextprotocol/server-filesystem ."
                  spellCheck={false}
                  onChange={(event) =>
                    setDraft({ ...draft, args: event.target.value })
                  }
                />
              </label>

              <p className="provider-form__hint">
                Quote any argument containing spaces. The id namespaces this
                server’s tools, so two servers can both expose a tool called
                “search”.
              </p>

              <div className="provider-form__actions">
                <div className="provider-form__spacer" />
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
                  {busy ? "Connecting…" : "Save & connect"}
                </button>
              </div>
            </section>
          )}
        </div>
      </div>
    </div>
  );
}
