import { useCallback, useEffect, useMemo, useState } from "react";
import {
  type Agent,
  type ProviderView,
  type Session,
  type ToolSpec,
  deleteSession,
  listAgents,
  listProviders,
  listSessions,
  listTools,
  renameSession,
} from "./lib/ipc";
import { ChatView } from "./features/chat/ChatView";
import { AgentsView } from "./features/agents/AgentsView";
import { ProviderSettings } from "./features/settings/ProviderSettings";
import { McpSettings } from "./features/tools/McpSettings";
import { DocumentSettings } from "./features/documents/DocumentSettings";
import { WindowControls } from "./WindowControls";
import "./App.css";

type Section = "chats" | "agents";

/** Remembers whether the sidebar was collapsed, across restarts. */
const SIDEBAR_KEY = "nexus.sidebar.collapsed";

function PanelIcon({ collapsed }: { collapsed: boolean }) {
  return (
    <svg width="15" height="15" viewBox="0 0 15 15" aria-hidden="true">
      <rect
        x="1.25"
        y="2.25"
        width="12.5"
        height="10.5"
        rx="2"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.2"
      />
      <line
        x1="5.75"
        y1="2.25"
        x2="5.75"
        y2="12.75"
        stroke="currentColor"
        strokeWidth="1.2"
      />
      {/* Filled when open, hollow when collapsed, so the icon shows state
          rather than only the action. */}
      {!collapsed && (
        <rect x="1.85" y="2.85" width="3.3" height="9.3" fill="currentColor" />
      )}
    </svg>
  );
}

function formatWhen(unixSeconds: number) {
  const date = new Date(unixSeconds * 1000);
  const sameDay = new Date().toDateString() === date.toDateString();
  return sameDay
    ? date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
    : date.toLocaleDateString([], { month: "short", day: "numeric" });
}

export default function App() {
  const [section, setSection] = useState<Section>("chats");
  const [sidebarCollapsed, setSidebarCollapsed] = useState(
    () => localStorage.getItem(SIDEBAR_KEY) === "true",
  );

  const toggleSidebar = useCallback(() => {
    setSidebarCollapsed((collapsed) => {
      const next = !collapsed;
      localStorage.setItem(SIDEBAR_KEY, String(next));
      return next;
    });
  }, []);

  // Ctrl/Cmd+B, the convention in editors and every app with a side panel.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "b") {
        event.preventDefault();
        toggleSidebar();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleSidebar]);

  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [tools, setTools] = useState<ToolSpec[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);

  /** The open conversation. `null` is an unsaved new chat. */
  const [session, setSession] = useState<Session | null>(null);
  /** Agent for a new chat that has no session row yet. */
  const [pendingAgent, setPendingAgent] = useState<Agent | null>(null);

  /**
   * Identity of the open conversation for remount purposes.
   *
   * Deliberately not the session id: a new chat gets its session row on the
   * first send, and keying on the id would remount the pane mid-stream,
   * discarding the run and the messages with it.
   */
  const [chatKey, setChatKey] = useState<string>(() => crypto.randomUUID());

  const [settingsPane, setSettingsPane] = useState<
    "providers" | "mcp" | "documents" | null
  >(null);

  const refreshProviders = useCallback(async () => {
    try {
      setProviders(await listProviders());
    } catch (error: unknown) {
      console.error("failed to load providers", error);
    }
  }, []);

  const refreshAgents = useCallback(async () => {
    try {
      setAgents(await listAgents());
    } catch (error: unknown) {
      console.error("failed to load agents", error);
    }
  }, []);

  const refreshTools = useCallback(async () => {
    try {
      setTools(await listTools());
    } catch (error: unknown) {
      console.error("failed to load tools", error);
    }
  }, []);

  const refreshSessions = useCallback(async () => {
    try {
      setSessions(await listSessions());
    } catch (error: unknown) {
      console.error("failed to load sessions", error);
    }
  }, []);

  // Initial load, guarded so a result arriving after unmount is discarded.
  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const [loadedProviders, loadedAgents, loadedTools, loadedSessions] =
        await Promise.all([
          listProviders().catch(() => [] as ProviderView[]),
          listAgents().catch(() => [] as Agent[]),
          listTools().catch(() => [] as ToolSpec[]),
          listSessions().catch(() => [] as Session[]),
        ]);

      if (cancelled) return;
      setProviders(loadedProviders);
      setAgents(loadedAgents);
      setTools(loadedTools);
      setSessions(loadedSessions);
    })();

    // MCP servers connect in the background after launch, so the first tool
    // list can be short-lived.
    const timer = setTimeout(() => {
      void listTools().then((loaded) => {
        if (!cancelled) setTools(loaded);
      });
    }, 2500);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, []);

  /** The agent for the open conversation, if it has one. */
  const activeAgent = useMemo(() => {
    if (session) {
      return agents.find((agent) => agent.id === session.agentId) ?? null;
    }
    return pendingAgent;
  }, [session, agents, pendingAgent]);

  const startNewChat = useCallback((agent: Agent | null) => {
    setSession(null);
    setPendingAgent(agent);
    setChatKey(crypto.randomUUID());
    setSection("chats");
  }, []);

  const openSession = useCallback((target: Session) => {
    setSession(target);
    setPendingAgent(null);
    setChatKey(target.id);
  }, []);

  const removeSession = useCallback(
    async (target: Session) => {
      try {
        await deleteSession(target.id);
        // Deleting the open conversation drops back to a fresh one; the key
        // changes so the pane resets rather than showing the deleted
        // transcript.
        if (session?.id === target.id) startNewChat(null);
        await refreshSessions();
      } catch (error: unknown) {
        console.error("failed to delete session", error);
      }
    },
    [session, refreshSessions, startNewChat],
  );

  return (
    <div className="shell">
      <header className="titlebar" data-tauri-drag-region>
        {/* In the titlebar rather than the sidebar so it stays reachable
            when the sidebar is gone. */}
        <button
          type="button"
          className="titlebar__toggle"
          onClick={toggleSidebar}
          aria-label={sidebarCollapsed ? "Show sidebar" : "Hide sidebar"}
          aria-pressed={!sidebarCollapsed}
          title={`${sidebarCollapsed ? "Show" : "Hide"} sidebar  (Ctrl+B)`}
        >
          <PanelIcon collapsed={sidebarCollapsed} />
        </button>
        <span className="titlebar__name">Nexus</span>
        <WindowControls />
      </header>

      <div className={`body ${sidebarCollapsed ? "body--collapsed" : ""}`}>
        <aside className="sidebar">
          <nav className="nav">
            <button
              type="button"
              className={`nav__item ${section === "chats" ? "nav__item--active" : ""}`}
              onClick={() => setSection("chats")}
            >
              Chats
            </button>
            <button
              type="button"
              className={`nav__item ${section === "agents" ? "nav__item--active" : ""}`}
              onClick={() => setSection("agents")}
            >
              Agents
              {agents.length > 0 && (
                <span className="nav__count">{agents.length}</span>
              )}
            </button>
          </nav>

          {section === "chats" && (
            <>
              <button
                type="button"
                className="button button--send sidebar__new"
                onClick={() => startNewChat(null)}
              >
                New chat
              </button>

              <div className="sidebar__list">
                {sessions.length === 0 && (
                  <p className="sidebar__empty">No conversations yet.</p>
                )}
                {sessions.map((item) => {
                  const agent = agents.find(
                    (candidate) => candidate.id === item.agentId,
                  );
                  return (
                    <div
                      key={item.id}
                      className={`session ${
                        item.id === session?.id ? "session--active" : ""
                      }`}
                    >
                      <button
                        type="button"
                        className="session__open"
                        onClick={() => openSession(item)}
                        onDoubleClick={() => {
                          const next = window.prompt("Rename chat", item.title);
                          if (next?.trim()) {
                            void renameSession(item.id, next.trim()).then(
                              refreshSessions,
                            );
                          }
                        }}
                        title={item.title}
                      >
                        <span className="session__title">{item.title}</span>
                        <span className="session__meta">
                          {agent ? `${agent.name} · ` : ""}
                          {formatWhen(item.updatedAt)}
                        </span>
                      </button>
                      <button
                        type="button"
                        className="session__delete"
                        aria-label={`Delete ${item.title}`}
                        onClick={() => void removeSession(item)}
                      >
                        ×
                      </button>
                    </div>
                  );
                })}
              </div>
            </>
          )}

          {section === "agents" && (
            <div className="sidebar__list">
              <p className="sidebar__empty">
                Agents bundle instructions, a model and tools.
              </p>
            </div>
          )}

          <footer className="sidebar__footer">
            <button
              type="button"
              className="nav__item nav__item--small"
              onClick={() => setSettingsPane("providers")}
            >
              Providers
            </button>
            <button
              type="button"
              className="nav__item nav__item--small"
              onClick={() => setSettingsPane("mcp")}
            >
              MCP
            </button>
            <button
              type="button"
              className="nav__item nav__item--small"
              onClick={() => setSettingsPane("documents")}
            >
              Documents
            </button>
          </footer>
        </aside>

        <div className="main">
          {section === "chats" ? (
            <ChatView
              // Remount only when the user switches conversation, never when
              // the current one acquires its session row.
              key={chatKey}
              session={session}
              agent={activeAgent}
              providers={providers}
              onSessionCreated={(created) => {
                setSession(created);
                setPendingAgent(null);
                void refreshSessions();
              }}
              onSessionsChanged={() => void refreshSessions()}
              onOpenSettings={() => setSettingsPane("providers")}
            />
          ) : (
            <AgentsView
              agents={agents}
              providers={providers}
              tools={tools}
              onChanged={() => void refreshAgents()}
              onStartChat={startNewChat}
            />
          )}
        </div>
      </div>

      {settingsPane === "providers" && (
        <ProviderSettings
          onClose={() => setSettingsPane(null)}
          onChanged={() => void refreshProviders()}
        />
      )}

      {settingsPane === "mcp" && (
        <McpSettings
          onClose={() => setSettingsPane(null)}
          onChanged={() => void refreshTools()}
        />
      )}

      {settingsPane === "documents" && (
        <DocumentSettings
          onClose={() => setSettingsPane(null)}
          onChanged={() => void refreshTools()}
        />
      )}
    </div>
  );
}
