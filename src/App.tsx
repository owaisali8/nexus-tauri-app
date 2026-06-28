import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type ProviderKind = "cloud" | "local";

type LlmProvider = {
  id: string;
  name: string;
  kind: ProviderKind;
  status: "configured" | "needs_key" | "local_required";
  models: string[];
};

type AgentProfile = {
  id: string;
  name: string;
  description: string;
  systemInstructions: string;
  providerId: string;
  model: string;
  tools: string[];
  mcps: string[];
  skills: string[];
};

type WorkspaceFolder = {
  id: string;
  name: string;
  items: number;
};

type Note = {
  id: string;
  folderId: string;
  title: string;
  body: string;
  updatedAt: number;
};

type BrowserRun = {
  id: string;
  agentId: string;
  targetUrl: string;
  objective: string;
  resumeFileName: string;
  status: "draft" | "ready" | "blocked";
};

type RuntimePlan = {
  agentId: string;
  providerId: string;
  model: string;
  rigProvider: string;
  ready: boolean;
  missingConfiguration: string[];
  tools: string[];
  mcps: string[];
  skills: string[];
};

type AgentPromptResponse = {
  agentId: string;
  providerId: string;
  model: string;
  output: string;
  runtimePlan: RuntimePlan;
};

type AppSnapshot = {
  providers: LlmProvider[];
  agents: AgentProfile[];
  folders: WorkspaceFolder[];
  notes: Note[];
  browserRuns: BrowserRun[];
  rigMarker: string;
};

const defaultSnapshot: AppSnapshot = {
  providers: [],
  agents: [],
  folders: [],
  notes: [],
  browserRuns: [],
  rigMarker: "",
};

function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot>(defaultSnapshot);
  const [selectedAgentId, setSelectedAgentId] = useState("job-application-agent");
  const [objective, setObjective] = useState(
    "Apply to frontend engineer roles, tailor answers from my resume, and stop before final submission.",
  );
  const [targetUrl, setTargetUrl] = useState("https://www.linkedin.com/jobs/");
  const [resumeFileName, setResumeFileName] = useState("Owais_Resume.pdf");
  const [runStatus, setRunStatus] = useState("");
  const [agentDraft, setAgentDraft] = useState<AgentProfile | null>(null);
  const [runtimePlan, setRuntimePlan] = useState<RuntimePlan | null>(null);
  const [llmPrompt, setLlmPrompt] = useState("Summarize your current role and what tools you can use.");
  const [llmOutput, setLlmOutput] = useState("");
  const [isPromptRunning, setIsPromptRunning] = useState(false);

  useEffect(() => {
    refreshSnapshot()
      .catch((error) => {
        setRunStatus(`Core unavailable: ${String(error)}`);
      });
  }, []);

  useEffect(() => {
    const agent = snapshot.agents.find((item) => item.id === selectedAgentId) ?? null;
    setAgentDraft(agent ? { ...agent } : null);

    if (agent) {
      invoke<RuntimePlan>("get_runtime_plan", { agentId: agent.id })
        .then(setRuntimePlan)
        .catch((error) => setRunStatus(`Runtime unavailable: ${String(error)}`));
    }
  }, [selectedAgentId, snapshot.agents]);

  async function refreshSnapshot() {
    await invoke<AppSnapshot>("get_app_snapshot")
      .then((data) => {
        setSnapshot(data);
        setSelectedAgentId(data.agents[0]?.id ?? selectedAgentId);
      });
  }

  const selectedAgent = useMemo(
    () => snapshot.agents.find((agent) => agent.id === selectedAgentId),
    [snapshot.agents, selectedAgentId],
  );

  const selectedProvider = useMemo(
    () => snapshot.providers.find((provider) => provider.id === selectedAgent?.providerId),
    [snapshot.providers, selectedAgent],
  );

  async function prepareBrowserRun() {
    setRunStatus("Preparing browser workflow...");
    try {
      const response = await invoke<BrowserRun>("prepare_browser_run", {
        request: {
          agentId: selectedAgentId,
          targetUrl,
          objective,
          resumeFileName,
        },
      });
      setRunStatus(`Run ${response.id} is ${response.status}. CDP controller is staged.`);
      await refreshSnapshot();
    } catch (error) {
      setRunStatus(`Could not prepare run: ${String(error)}`);
    }
  }

  async function saveAgentDraft() {
    if (!agentDraft) return;

    try {
      const saved = await invoke<AgentProfile>("save_agent", { agent: agentDraft });
      setRunStatus(`Saved ${saved.name}.`);
      await refreshSnapshot();
    } catch (error) {
      setRunStatus(`Could not save agent: ${String(error)}`);
    }
  }

  async function saveQuickNote() {
    const note: Note = {
      id: `note-${Date.now()}`,
      folderId: "job-research",
      title: "Browser run note",
      body: `Target: ${targetUrl}\nObjective: ${objective}`,
      updatedAt: Math.floor(Date.now() / 1000),
    };

    try {
      await invoke<Note>("save_note", { note });
      setRunStatus("Saved note to Job Research.");
      await refreshSnapshot();
    } catch (error) {
      setRunStatus(`Could not save note: ${String(error)}`);
    }
  }

  async function runAgentPrompt() {
    if (!selectedAgentId || !llmPrompt.trim()) return;

    setIsPromptRunning(true);
    setLlmOutput("");
    setRunStatus("Running agent prompt...");
    try {
      const response = await invoke<AgentPromptResponse>("run_agent_prompt", {
        request: {
          agentId: selectedAgentId,
          prompt: llmPrompt,
        },
      });
      setRuntimePlan(response.runtimePlan);
      setLlmOutput(response.output);
      setRunStatus(`Completed with ${response.providerId} / ${response.model}.`);
    } catch (error) {
      setRunStatus(`Prompt failed: ${String(error)}`);
    } finally {
      setIsPromptRunning(false);
    }
  }

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
          {["Agents", "Browser", "Notes", "Files", "MCP", "Settings"].map((item, index) => (
            <button className={index === 1 ? "nav-item active" : "nav-item"} key={item}>
              <span>{item.slice(0, 1)}</span>
              {item}
            </button>
          ))}
        </nav>

        <section className="sidebar-section">
          <div className="section-label">Providers</div>
          <div className="provider-list">
            {snapshot.providers.map((provider) => (
              <div className="provider-row" key={provider.id}>
                <span className={`status-dot ${provider.kind}`} />
                <div>
                  <strong>{provider.name}</strong>
                  <small>{provider.models.length} models</small>
                </div>
              </div>
            ))}
          </div>
        </section>
      </aside>

      <section className="workspace">
        <header className="topbar">
          <div>
            <p className="eyebrow">Browser automation</p>
            <h2>Run a job application agent with local-first control</h2>
          </div>
          <div className="topbar-actions">
            <button className="icon-button" aria-label="Open command menu">/</button>
            <button className="secondary-button" onClick={saveQuickNote}>Save note</button>
            <button className="primary-button" onClick={prepareBrowserRun}>Prepare run</button>
          </div>
        </header>

        <div className="content-grid">
          <section className="panel agent-panel">
            <div className="panel-header">
              <div>
                <p className="eyebrow">Agent profile</p>
                <h3>{selectedAgent?.name ?? "No agent"}</h3>
              </div>
              <select value={selectedAgentId} onChange={(event) => setSelectedAgentId(event.target.value)}>
                {snapshot.agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>{agent.name}</option>
                ))}
              </select>
            </div>

            <label className="field">
              <span>System instructions</span>
              <textarea
                value={agentDraft?.systemInstructions ?? ""}
                onChange={(event) => {
                  if (!agentDraft) return;
                  setAgentDraft({ ...agentDraft, systemInstructions: event.target.value });
                }}
              />
            </label>

            <div className="form-grid">
              <label className="field">
                <span>Provider</span>
                <select
                  value={agentDraft?.providerId ?? ""}
                  onChange={(event) => {
                    if (!agentDraft) return;
                    const provider = snapshot.providers.find((item) => item.id === event.target.value);
                    setAgentDraft({
                      ...agentDraft,
                      providerId: event.target.value,
                      model: provider?.models[0] ?? agentDraft.model,
                    });
                  }}
                >
                  {snapshot.providers.map((provider) => (
                    <option key={provider.id} value={provider.id}>{provider.name}</option>
                  ))}
                </select>
              </label>
              <label className="field">
                <span>Model</span>
                <input
                  value={agentDraft?.model ?? ""}
                  onChange={(event) => {
                    if (!agentDraft) return;
                    setAgentDraft({ ...agentDraft, model: event.target.value });
                  }}
                />
              </label>
            </div>

            <div className="chip-group">
              {selectedAgent?.tools.map((tool) => <span className="chip" key={tool}>{tool}</span>)}
              {selectedAgent?.mcps.map((mcp) => <span className="chip muted" key={mcp}>{mcp}</span>)}
              {selectedAgent?.skills.map((skill) => <span className="chip muted" key={skill}>{skill}</span>)}
            </div>

            <div className="panel-actions">
              <button className="secondary-button" onClick={saveAgentDraft}>Save agent</button>
            </div>
          </section>

          <section className="panel run-panel">
            <div className="panel-header">
              <div>
                <p className="eyebrow">Workflow</p>
                <h3>Job application run</h3>
              </div>
              <span className="badge">Human approval before submit</span>
            </div>

            <div className="form-grid">
              <label className="field">
                <span>Target URL</span>
                <input value={targetUrl} onChange={(event) => setTargetUrl(event.target.value)} />
              </label>
              <label className="field">
                <span>Resume file</span>
                <input value={resumeFileName} onChange={(event) => setResumeFileName(event.target.value)} />
              </label>
            </div>

            <label className="field">
              <span>Objective</span>
              <textarea value={objective} onChange={(event) => setObjective(event.target.value)} />
            </label>

            <div className="browser-preview">
              <div className="browser-bar">
                <span />
                <span />
                <span />
                <strong>{targetUrl.replace("https://", "")}</strong>
              </div>
              <div className="browser-body">
                <div className="job-card">
                  <small>Detected step</small>
                  <strong>Resume upload and screening questions</strong>
                  <p>Agent can navigate, fill fields, attach PDF files, and request approval before final submission.</p>
                </div>
              </div>
            </div>

            {runStatus && <p className="run-status">{runStatus}</p>}
          </section>

          <section className="panel llm-panel">
            <div className="panel-header">
              <div>
                <p className="eyebrow">LLM execution</p>
                <h3>Run agent prompt</h3>
              </div>
              <span className={runtimePlan?.ready ? "ready-pill" : "blocked-pill"}>
                {runtimePlan?.ready ? "Ready" : "Needs config"}
              </span>
            </div>

            <label className="field">
              <span>Prompt</span>
              <textarea value={llmPrompt} onChange={(event) => setLlmPrompt(event.target.value)} />
            </label>

            <div className="panel-actions">
              <button className="primary-button" disabled={isPromptRunning} onClick={runAgentPrompt}>
                {isPromptRunning ? "Running..." : "Run prompt"}
              </button>
            </div>

            {llmOutput && (
              <div className="llm-output">
                <strong>Response</strong>
                <p>{llmOutput}</p>
              </div>
            )}
          </section>

          <section className="panel compact-panel">
            <p className="eyebrow">Model route</p>
            <h3>{selectedProvider?.name ?? "Unassigned"}</h3>
            <p>{runtimePlan?.rigProvider ?? selectedAgent?.model ?? "No model selected"}</p>
            <div className="runtime-row">
              <span className={runtimePlan?.ready ? "ready-pill" : "blocked-pill"}>
                {runtimePlan?.ready ? "Ready" : "Needs config"}
              </span>
              <small>{runtimePlan?.missingConfiguration.join(", ") || snapshot.rigMarker}</small>
            </div>
            <div className="meter"><span style={{ width: runtimePlan?.ready ? "100%" : "42%" }} /></div>
          </section>

          <section className="panel compact-panel">
            <p className="eyebrow">Local knowledge</p>
            <h3>{snapshot.folders.reduce((total, folder) => total + folder.items, 0) + snapshot.notes.length} items</h3>
            <div className="folder-list">
              {snapshot.folders.map((folder) => (
                <div key={folder.id}>
                  <strong>{folder.name}</strong>
                  <span>{folder.items}</span>
                </div>
              ))}
            </div>
          </section>

          <section className="panel compact-panel">
            <p className="eyebrow">Saved runs</p>
            <h3>{snapshot.browserRuns.length} runs</h3>
            <div className="folder-list">
              {snapshot.browserRuns.slice(0, 3).map((run) => (
                <div key={run.id}>
                  <strong>{run.targetUrl.replace("https://", "")}</strong>
                  <span>{run.status}</span>
                </div>
              ))}
            </div>
          </section>

          <section className="panel compact-panel">
            <p className="eyebrow">Notes</p>
            <h3>{snapshot.notes.length} saved</h3>
            <div className="folder-list">
              {snapshot.notes.slice(0, 3).map((note) => (
                <div key={note.id}>
                  <strong>{note.title}</strong>
                  <span>{note.folderId}</span>
                </div>
              ))}
            </div>
          </section>
        </div>
      </section>
    </main>
  );
}

export default App;
