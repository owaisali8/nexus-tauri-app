import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  AgentProfile,
  AgentSession,
  AppSnapshot,
  AppView,
  ChatMessageResponse,
  DatabaseInfo,
  FileRecord,
  LlmProvider,
  MemoryEntry,
  RuntimePlan,
  SessionMessage,
  StoredProvider,
} from "./types";
import "./App.css";

const defaultSnapshot: AppSnapshot = {
  providers: [],
  providerTemplates: [],
  agents: [],
  folders: [],
  notes: [],
  fileRecords: [],
  sessions: [],
  memoryEntries: [],
  browserRuns: [],
  rigMarker: "",
};

function configuredProviders(providers: LlmProvider[]) {
  return providers.filter((provider) => provider.status === "configured");
}

function slugify(value: string) {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
}

function formatTime(timestamp: number) {
  if (!timestamp) return "just now";
  return new Date(timestamp * 1000).toLocaleString();
}

function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot>(defaultSnapshot);
  const [view, setView] = useState<AppView>("providers");
  const [status, setStatus] = useState("");
  const [databaseInfo, setDatabaseInfo] = useState<DatabaseInfo | null>(null);

  const [selectedTemplateId, setSelectedTemplateId] = useState("");
  const [providerDraft, setProviderDraft] = useState<StoredProvider | null>(null);

  const [selectedAgentId, setSelectedAgentId] = useState("");
  const [agentDraft, setAgentDraft] = useState<AgentProfile | null>(null);
  const [runtimePlan, setRuntimePlan] = useState<RuntimePlan | null>(null);

  const [selectedSessionId, setSelectedSessionId] = useState("");
  const [messages, setMessages] = useState<SessionMessage[]>([]);
  const [chatInput, setChatInput] = useState("");
  const [isSending, setIsSending] = useState(false);

  const [fileDraft, setFileDraft] = useState({
    name: "",
    path: "",
    mimeType: "application/pdf",
    summary: "",
    folderId: "resumes",
  });

  const [memoryDraft, setMemoryDraft] = useState({
    key: "",
    value: "",
  });

  const chatEndRef = useRef<HTMLDivElement | null>(null);

  const readyProviders = useMemo(
    () => configuredProviders(snapshot.providers),
    [snapshot.providers],
  );

  const selectedAgent = useMemo(
    () => snapshot.agents.find((agent) => agent.id === selectedAgentId) ?? null,
    [snapshot.agents, selectedAgentId],
  );

  const agentSessions = useMemo(
    () =>
      snapshot.sessions
        .filter((session) => session.agentId === selectedAgentId)
        .sort((left, right) => right.updatedAt - left.updatedAt),
    [snapshot.sessions, selectedAgentId],
  );

  const agentMemory = useMemo(
    () => snapshot.memoryEntries.filter((entry) => entry.agentId === selectedAgentId),
    [snapshot.memoryEntries, selectedAgentId],
  );

  useEffect(() => {
    refreshSnapshot().catch((error) => {
      setStatus(`Core unavailable: ${String(error)}`);
    });
  }, []);

  useEffect(() => {
    if (readyProviders.length === 0) {
      setView("providers");
      return;
    }
    if (snapshot.agents.length === 0 && (view === "chat" || view === "memory")) {
      setView("agents");
    }
  }, [readyProviders.length, snapshot.agents.length, view]);

  useEffect(() => {
    if (!selectedTemplateId && snapshot.providerTemplates[0]) {
      setSelectedTemplateId(snapshot.providerTemplates[0].id);
    }
  }, [snapshot.providerTemplates, selectedTemplateId]);

  useEffect(() => {
    const template = snapshot.providerTemplates.find((item) => item.id === selectedTemplateId);
    if (!template) return;

    const existing = snapshot.providers.find((provider) => provider.id === template.id);
    if (existing) {
      setProviderDraft({
        id: existing.id,
        name: existing.name,
        kind: existing.kind,
        apiKey: existing.hasApiKey ? "configured" : "",
        baseUrl: existing.baseUrl ?? template.defaultBaseUrl ?? "",
        models: existing.models,
        isEnabled: true,
      });
      return;
    }

    setProviderDraft({
      id: template.id,
      name: template.name,
      kind: template.kind,
      apiKey: "",
      baseUrl: template.defaultBaseUrl ?? "",
      models: template.defaultModels,
      isEnabled: true,
    });
  }, [selectedTemplateId, snapshot.providerTemplates, snapshot.providers]);

  useEffect(() => {
    if (!selectedAgentId && snapshot.agents[0]) {
      setSelectedAgentId(snapshot.agents[0].id);
    }
  }, [snapshot.agents, selectedAgentId]);

  useEffect(() => {
    const agent = snapshot.agents.find((item) => item.id === selectedAgentId) ?? null;
    setAgentDraft(
      agent
        ? { ...agent }
        : {
            id: "",
            name: "",
            description: "",
            systemInstructions:
              "You are a helpful local-first assistant. Use saved files and memory when relevant.",
            providerId: readyProviders[0]?.id ?? "",
            model: readyProviders[0]?.models[0] ?? "",
            tools: [],
            mcps: [],
            skills: [],
          },
    );

    if (agent) {
      invoke<RuntimePlan>("get_runtime_plan", { agentId: agent.id })
        .then(setRuntimePlan)
        .catch((error) => setStatus(`Runtime unavailable: ${String(error)}`));
    } else {
      setRuntimePlan(null);
    }
  }, [selectedAgentId, snapshot.agents, readyProviders]);

  useEffect(() => {
    if (!selectedSessionId && agentSessions[0]) {
      setSelectedSessionId(agentSessions[0].id);
    }
  }, [agentSessions, selectedSessionId]);

  useEffect(() => {
    if (!selectedSessionId) {
      setMessages([]);
      return;
    }

    invoke<SessionMessage[]>("get_session_messages", { sessionId: selectedSessionId })
      .then(setMessages)
      .catch((error) => setStatus(`Could not load messages: ${String(error)}`));
  }, [selectedSessionId]);

  useEffect(() => {
    chatEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, isSending]);

  async function refreshSnapshot() {
    const data = await invoke<AppSnapshot>("get_app_snapshot");
    setSnapshot(data);
    setDatabaseInfo(await invoke<DatabaseInfo>("get_database_info"));
    if (!selectedAgentId && data.agents[0]) {
      setSelectedAgentId(data.agents[0].id);
    }
  }

  async function saveProvider() {
    if (!providerDraft) return;

    const payload: StoredProvider = {
      ...providerDraft,
      apiKey:
        providerDraft.apiKey === "configured"
          ? undefined
          : providerDraft.apiKey?.trim() || undefined,
      baseUrl: providerDraft.baseUrl?.trim() || undefined,
    };

    if (payload.kind === "cloud" && !payload.apiKey) {
      setStatus("Add an API key for cloud providers.");
      return;
    }

    try {
      await invoke("save_provider", { provider: payload });
      setStatus(`Saved ${payload.name}.`);
      await refreshSnapshot();
      if (snapshot.agents.length === 0) {
        setView("agents");
      }
    } catch (error) {
      setStatus(`Could not save provider: ${String(error)}`);
    }
  }

  async function saveAgent() {
    if (!agentDraft) return;

    const payload: AgentProfile = {
      ...agentDraft,
      id: agentDraft.id || `agent-${slugify(agentDraft.name || "custom")}-${Date.now()}`,
    };

    try {
      const saved = await invoke<AgentProfile>("save_agent", { agent: payload });
      setStatus(`Saved ${saved.name}.`);
      setSelectedAgentId(saved.id);
      await refreshSnapshot();
      setView("chat");
    } catch (error) {
      setStatus(`Could not save agent: ${String(error)}`);
    }
  }

  async function createSession() {
    if (!selectedAgentId) return;

    try {
      const session = await invoke<AgentSession>("create_agent_session", {
        request: { agentId: selectedAgentId },
      });
      setSelectedSessionId(session.id);
      setMessages([]);
      await refreshSnapshot();
      setView("chat");
      setStatus(`Started ${session.title}.`);
    } catch (error) {
      setStatus(`Could not create session: ${String(error)}`);
    }
  }

  async function sendMessage() {
    if (!selectedSessionId || !chatInput.trim()) return;

    setIsSending(true);
    const content = chatInput.trim();
    setChatInput("");

    try {
      const response = await invoke<ChatMessageResponse>("send_chat_message", {
        request: { sessionId: selectedSessionId, content },
      });
      setMessages((current) => [
        ...current,
        response.userMessage,
        response.assistantMessage,
      ]);
      setStatus("Reply received.");
      await refreshSnapshot();
    } catch (error) {
      setChatInput(content);
      setStatus(`Chat failed: ${String(error)}`);
    } finally {
      setIsSending(false);
    }
  }

  async function saveFileRecord() {
    if (!fileDraft.name.trim() || !fileDraft.path.trim()) {
      setStatus("File name and path are required.");
      return;
    }

    const record: FileRecord = {
      id: `file-${Date.now()}`,
      name: fileDraft.name.trim(),
      path: fileDraft.path.trim(),
      mimeType: fileDraft.mimeType.trim() || "application/octet-stream",
      sizeBytes: 0,
      folderId: fileDraft.folderId,
      summary: fileDraft.summary.trim(),
      createdAt: Math.floor(Date.now() / 1000),
      updatedAt: Math.floor(Date.now() / 1000),
    };

    try {
      await invoke("save_file_record", { file: record });
      setStatus(`Saved file record ${record.name}.`);
      setFileDraft((current) => ({ ...current, name: "", path: "", summary: "" }));
      await refreshSnapshot();
    } catch (error) {
      setStatus(`Could not save file: ${String(error)}`);
    }
  }

  async function saveMemoryEntry() {
    if (!selectedAgentId || !memoryDraft.key.trim() || !memoryDraft.value.trim()) {
      setStatus("Select an agent and provide memory key/value.");
      return;
    }

    const entry: MemoryEntry = {
      id: `memory-${Date.now()}`,
      agentId: selectedAgentId,
      key: memoryDraft.key.trim(),
      value: memoryDraft.value.trim(),
      source: "user",
      createdAt: Math.floor(Date.now() / 1000),
      updatedAt: Math.floor(Date.now() / 1000),
    };

    try {
      await invoke("save_memory_entry", { entry });
      setStatus("Saved long-term memory.");
      setMemoryDraft({ key: "", value: "" });
      await refreshSnapshot();
    } catch (error) {
      setStatus(`Could not save memory: ${String(error)}`);
    }
  }

  const navItems: { id: AppView; label: string; enabled: boolean }[] = [
    { id: "providers", label: "Providers", enabled: true },
    { id: "agents", label: "Agents", enabled: readyProviders.length > 0 },
    { id: "chat", label: "Chat", enabled: snapshot.agents.length > 0 },
    { id: "files", label: "Files", enabled: true },
    { id: "memory", label: "Memory", enabled: snapshot.agents.length > 0 },
  ];

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">e</div>
          <div>
            <h1>essentio</h1>
            <p>agent operating system</p>
          </div>
        </div>

        <nav className="nav-list" aria-label="Primary">
          {navItems.map((item) => (
            <button
              className={view === item.id ? "nav-item active" : "nav-item"}
              disabled={!item.enabled}
              key={item.id}
              onClick={() => item.enabled && setView(item.id)}
            >
              <span>{item.label.slice(0, 1)}</span>
              {item.label}
            </button>
          ))}
        </nav>

        <section className="sidebar-section">
          <div className="section-label">Setup progress</div>
          <div className="setup-steps">
            <div className={readyProviders.length > 0 ? "setup-step done" : "setup-step active"}>
              <strong>1. Providers</strong>
              <small>{readyProviders.length} configured</small>
            </div>
            <div className={snapshot.agents.length > 0 ? "setup-step done" : "setup-step"}>
              <strong>2. Agents</strong>
              <small>{snapshot.agents.length} created</small>
            </div>
            <div className={agentSessions.length > 0 ? "setup-step done" : "setup-step"}>
              <strong>3. Sessions</strong>
              <small>{agentSessions.length} for current agent</small>
            </div>
          </div>
        </section>

        <section className="sidebar-section">
          <div className="section-label">Providers</div>
          <div className="provider-list">
            {snapshot.providers.map((provider) => (
              <div className="provider-row" key={provider.id}>
                <span className={`status-dot ${provider.kind} ${provider.status}`} />
                <div>
                  <strong>{provider.name}</strong>
                  <small>{provider.status.replace(/_/g, " ")}</small>
                </div>
              </div>
            ))}
          </div>
        </section>
      </aside>

      <section className="workspace">
        <header className="topbar">
          <div>
            <p className="eyebrow">{view}</p>
            <h2>
              {view === "providers" && "Connect your LLM providers"}
              {view === "agents" && "Create a custom agent"}
              {view === "chat" && "Session chat"}
              {view === "files" && "File records"}
              {view === "memory" && "Long-term memory"}
            </h2>
          </div>
          <div className="topbar-actions">
            {view === "chat" && (
              <button className="secondary-button" onClick={createSession}>
                New session
              </button>
            )}
          </div>
        </header>

        {status && <p className="run-status top-status">{status}</p>}

        {view === "providers" && (
          <div className="content-grid single-column">
            <section className="panel">
              <div className="panel-header">
                <div>
                  <p className="eyebrow">Step 1</p>
                  <h3>Add an LLM provider</h3>
                </div>
              </div>

              <div className="form-grid">
                <label className="field">
                  <span>Provider template</span>
                  <select
                    value={selectedTemplateId}
                    onChange={(event) => setSelectedTemplateId(event.target.value)}
                  >
                    {snapshot.providerTemplates.map((template) => (
                      <option key={template.id} value={template.id}>
                        {template.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="field">
                  <span>Display name</span>
                  <input
                    value={providerDraft?.name ?? ""}
                    onChange={(event) =>
                      providerDraft &&
                      setProviderDraft({ ...providerDraft, name: event.target.value })
                    }
                  />
                </label>
              </div>

              {providerDraft?.kind === "cloud" ? (
                <label className="field">
                  <span>API key</span>
                  <input
                    type="password"
                    placeholder={
                      providerDraft.apiKey === "configured"
                        ? "Key saved — enter to replace"
                        : "Paste your API key"
                    }
                    value={providerDraft.apiKey === "configured" ? "" : providerDraft.apiKey ?? ""}
                    onChange={(event) =>
                      setProviderDraft({ ...providerDraft, apiKey: event.target.value })
                    }
                  />
                </label>
              ) : (
                <label className="field">
                  <span>Base URL</span>
                  <input
                    value={providerDraft?.baseUrl ?? ""}
                    onChange={(event) =>
                      providerDraft &&
                      setProviderDraft({ ...providerDraft, baseUrl: event.target.value })
                    }
                  />
                </label>
              )}

              <label className="field">
                <span>Models (comma separated)</span>
                <input
                  value={providerDraft?.models.join(", ") ?? ""}
                  onChange={(event) =>
                    providerDraft &&
                    setProviderDraft({
                      ...providerDraft,
                      models: event.target.value
                        .split(",")
                        .map((model) => model.trim())
                        .filter(Boolean),
                    })
                  }
                />
              </label>

              <div className="panel-actions">
                <button className="primary-button" onClick={saveProvider}>
                  Save provider
                </button>
              </div>
            </section>

            <section className="panel compact-panel">
              <p className="eyebrow">Configured</p>
              <h3>{readyProviders.length} ready</h3>
              <div className="folder-list">
                {snapshot.providers.map((provider) => (
                  <div key={provider.id}>
                    <strong>{provider.name}</strong>
                    <span>{provider.status}</span>
                  </div>
                ))}
              </div>
            </section>
          </div>
        )}

        {view === "agents" && (
          <div className="content-grid">
            <section className="panel agent-panel">
              <div className="panel-header">
                <div>
                  <p className="eyebrow">Step 2</p>
                  <h3>{selectedAgent ? "Edit agent" : "Create agent"}</h3>
                </div>
                <select
                  value={selectedAgentId}
                  onChange={(event) => setSelectedAgentId(event.target.value)}
                >
                  <option value="">New agent</option>
                  {snapshot.agents.map((agent) => (
                    <option key={agent.id} value={agent.id}>
                      {agent.name}
                    </option>
                  ))}
                </select>
              </div>

              <label className="field">
                <span>Name</span>
                <input
                  value={agentDraft?.name ?? ""}
                  onChange={(event) =>
                    agentDraft && setAgentDraft({ ...agentDraft, name: event.target.value })
                  }
                />
              </label>

              <label className="field">
                <span>Description</span>
                <input
                  value={agentDraft?.description ?? ""}
                  onChange={(event) =>
                    agentDraft &&
                    setAgentDraft({ ...agentDraft, description: event.target.value })
                  }
                />
              </label>

              <label className="field">
                <span>System instructions</span>
                <textarea
                  value={agentDraft?.systemInstructions ?? ""}
                  onChange={(event) =>
                    agentDraft &&
                    setAgentDraft({ ...agentDraft, systemInstructions: event.target.value })
                  }
                />
              </label>

              <div className="form-grid">
                <label className="field">
                  <span>Provider</span>
                  <select
                    value={agentDraft?.providerId ?? ""}
                    onChange={(event) => {
                      if (!agentDraft) return;
                      const provider = readyProviders.find(
                        (item) => item.id === event.target.value,
                      );
                      setAgentDraft({
                        ...agentDraft,
                        providerId: event.target.value,
                        model: provider?.models[0] ?? agentDraft.model,
                      });
                    }}
                  >
                    {readyProviders.map((provider) => (
                      <option key={provider.id} value={provider.id}>
                        {provider.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="field">
                  <span>Model</span>
                  <select
                    value={agentDraft?.model ?? ""}
                    onChange={(event) =>
                      agentDraft && setAgentDraft({ ...agentDraft, model: event.target.value })
                    }
                  >
                    {(readyProviders.find((item) => item.id === agentDraft?.providerId)?.models ??
                      []
                    ).map((model) => (
                      <option key={model} value={model}>
                        {model}
                      </option>
                    ))}
                  </select>
                </label>
              </div>

              <div className="panel-actions">
                <button className="primary-button" onClick={saveAgent}>
                  Save agent
                </button>
              </div>
            </section>

            <section className="panel compact-panel">
              <p className="eyebrow">Runtime</p>
              <h3>{runtimePlan?.ready ? "Ready" : "Needs config"}</h3>
              <p>{runtimePlan?.model ?? "No model selected"}</p>
              <div className="runtime-row">
                <span className={runtimePlan?.ready ? "ready-pill" : "blocked-pill"}>
                  {runtimePlan?.ready ? "Ready" : "Blocked"}
                </span>
                <small>{runtimePlan?.missingConfiguration.join(", ") || snapshot.rigMarker}</small>
              </div>
            </section>
          </div>
        )}

        {view === "chat" && (
          <div className="chat-layout">
            <aside className="chat-sidebar panel">
              <div className="panel-header">
                <div>
                  <p className="eyebrow">Agent</p>
                  <h3>{selectedAgent?.name ?? "No agent"}</h3>
                </div>
              </div>
              <select
                value={selectedAgentId}
                onChange={(event) => {
                  setSelectedAgentId(event.target.value);
                  setSelectedSessionId("");
                }}
              >
                {snapshot.agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {agent.name}
                  </option>
                ))}
              </select>

              <div className="section-label session-label">Sessions</div>
              <div className="session-list">
                {agentSessions.map((session) => (
                  <button
                    className={
                      session.id === selectedSessionId ? "session-item active" : "session-item"
                    }
                    key={session.id}
                    onClick={() => setSelectedSessionId(session.id)}
                  >
                    <strong>{session.title}</strong>
                    <small>{formatTime(session.updatedAt)}</small>
                  </button>
                ))}
                {agentSessions.length === 0 && (
                  <p className="empty-copy">No sessions yet. Start one to chat.</p>
                )}
              </div>
            </aside>

            <section className="panel chat-panel">
              <div className="panel-header">
                <div>
                  <p className="eyebrow">Conversation</p>
                  <h3>
                    {agentSessions.find((session) => session.id === selectedSessionId)?.title ??
                      "Select or create a session"}
                  </h3>
                </div>
                <span className={runtimePlan?.ready ? "ready-pill" : "blocked-pill"}>
                  {runtimePlan?.ready ? "Ready" : "Needs config"}
                </span>
              </div>

              <div className="chat-thread">
                {messages.map((message) => (
                  <article
                    className={
                      message.role === "user" ? "chat-bubble user" : "chat-bubble assistant"
                    }
                    key={message.id}
                  >
                    <small>{message.role}</small>
                    <p>{message.content}</p>
                  </article>
                ))}
                {isSending && (
                  <article className="chat-bubble assistant pending">
                    <small>assistant</small>
                    <p>Thinking...</p>
                  </article>
                )}
                <div ref={chatEndRef} />
              </div>

              <div className="chat-composer">
                <textarea
                  placeholder={
                    selectedSessionId
                      ? "Message your agent..."
                      : "Create a session to start chatting"
                  }
                  value={chatInput}
                  disabled={!selectedSessionId || isSending}
                  onChange={(event) => setChatInput(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" && !event.shiftKey) {
                      event.preventDefault();
                      void sendMessage();
                    }
                  }}
                />
                <button
                  className="primary-button"
                  disabled={!selectedSessionId || isSending || !chatInput.trim()}
                  onClick={() => void sendMessage()}
                >
                  {isSending ? "Sending..." : "Send"}
                </button>
              </div>
            </section>
          </div>
        )}

        {view === "files" && (
          <div className="content-grid">
            <section className="panel">
              <div className="panel-header">
                <div>
                  <p className="eyebrow">Local files</p>
                  <h3>Register a file record</h3>
                </div>
              </div>

              <div className="form-grid">
                <label className="field">
                  <span>Name</span>
                  <input
                    value={fileDraft.name}
                    onChange={(event) =>
                      setFileDraft((current) => ({ ...current, name: event.target.value }))
                    }
                  />
                </label>
                <label className="field">
                  <span>Folder</span>
                  <select
                    value={fileDraft.folderId}
                    onChange={(event) =>
                      setFileDraft((current) => ({ ...current, folderId: event.target.value }))
                    }
                  >
                    {snapshot.folders.map((folder) => (
                      <option key={folder.id} value={folder.id}>
                        {folder.name}
                      </option>
                    ))}
                  </select>
                </label>
              </div>

              <label className="field">
                <span>Path</span>
                <input
                  value={fileDraft.path}
                  onChange={(event) =>
                    setFileDraft((current) => ({ ...current, path: event.target.value }))
                  }
                />
              </label>

              <label className="field">
                <span>Summary</span>
                <textarea
                  value={fileDraft.summary}
                  onChange={(event) =>
                    setFileDraft((current) => ({ ...current, summary: event.target.value }))
                  }
                />
              </label>

              <div className="panel-actions">
                <button className="primary-button" onClick={() => void saveFileRecord()}>
                  Save file record
                </button>
              </div>
            </section>

            <section className="panel compact-panel">
              <p className="eyebrow">Indexed files</p>
              <h3>{snapshot.fileRecords.length} records</h3>
              <div className="folder-list">
                {snapshot.fileRecords.map((file) => (
                  <div key={file.id}>
                    <strong>{file.name}</strong>
                    <span>{file.folderId ?? "unfiled"}</span>
                  </div>
                ))}
              </div>
            </section>
          </div>
        )}

        {view === "memory" && (
          <div className="content-grid">
            <section className="panel">
              <div className="panel-header">
                <div>
                  <p className="eyebrow">Agent memory</p>
                  <h3>Long-term memory for {selectedAgent?.name ?? "agent"}</h3>
                </div>
                <select value={selectedAgentId} onChange={(event) => setSelectedAgentId(event.target.value)}>
                  {snapshot.agents.map((agent) => (
                    <option key={agent.id} value={agent.id}>
                      {agent.name}
                    </option>
                  ))}
                </select>
              </div>

              <label className="field">
                <span>Key</span>
                <input
                  value={memoryDraft.key}
                  onChange={(event) =>
                    setMemoryDraft((current) => ({ ...current, key: event.target.value }))
                  }
                />
              </label>

              <label className="field">
                <span>Value</span>
                <textarea
                  value={memoryDraft.value}
                  onChange={(event) =>
                    setMemoryDraft((current) => ({ ...current, value: event.target.value }))
                  }
                />
              </label>

              <div className="panel-actions">
                <button className="primary-button" onClick={() => void saveMemoryEntry()}>
                  Save memory
                </button>
              </div>
            </section>

            <section className="panel compact-panel">
              <p className="eyebrow">Stored entries</p>
              <h3>{agentMemory.length} items</h3>
              <div className="memory-list">
                {agentMemory.map((entry) => (
                  <div className="memory-item" key={entry.id}>
                    <strong>{entry.key}</strong>
                    <p>{entry.value}</p>
                    <small>{entry.source}</small>
                  </div>
                ))}
              </div>
            </section>
          </div>
        )}

        <footer className="status-footer">
          <span>{databaseInfo?.path ?? "SQLite store"}</span>
          <span>
            {databaseInfo?.providerCount ?? 0} providers · {databaseInfo?.agentCount ?? 0} agents ·{" "}
            {databaseInfo?.sessionCount ?? 0} sessions · {databaseInfo?.fileCount ?? 0} files ·{" "}
            {databaseInfo?.memoryCount ?? 0} memory
          </span>
        </footer>
      </section>
    </main>
  );
}

export default App;
