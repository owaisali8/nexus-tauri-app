export type ProviderKind = "cloud" | "local";
export type ProviderStatus = "configured" | "needs_key" | "local_required";

export type LlmProvider = {
  id: string;
  name: string;
  kind: ProviderKind;
  status: ProviderStatus;
  models: string[];
  baseUrl?: string | null;
  hasApiKey: boolean;
};

export type ProviderTemplate = {
  id: string;
  name: string;
  kind: ProviderKind;
  defaultModels: string[];
  defaultBaseUrl?: string | null;
  requiresApiKey: boolean;
};

export type StoredProvider = {
  id: string;
  name: string;
  kind: ProviderKind;
  apiKey?: string | null;
  baseUrl?: string | null;
  models: string[];
  isEnabled: boolean;
};

export type AgentProfile = {
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

export type WorkspaceFolder = {
  id: string;
  name: string;
  items: number;
};

export type Note = {
  id: string;
  folderId: string;
  title: string;
  body: string;
  updatedAt: number;
};

export type FileRecord = {
  id: string;
  name: string;
  path: string;
  mimeType: string;
  sizeBytes: number;
  folderId?: string | null;
  summary: string;
  createdAt: number;
  updatedAt: number;
};

export type AgentSession = {
  id: string;
  agentId: string;
  title: string;
  createdAt: number;
  updatedAt: number;
};

export type SessionMessage = {
  id: string;
  sessionId: string;
  role: "user" | "assistant" | "system";
  content: string;
  createdAt: number;
};

export type MemoryEntry = {
  id: string;
  agentId: string;
  key: string;
  value: string;
  source: "user" | "agent" | "file";
  createdAt: number;
  updatedAt: number;
};

export type BrowserRun = {
  id: string;
  agentId: string;
  targetUrl: string;
  objective: string;
  resumeFileName: string;
  status: "draft" | "ready" | "blocked";
};

export type RuntimePlan = {
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

export type DatabaseInfo = {
  path: string;
  providerCount: number;
  agentCount: number;
  noteCount: number;
  fileCount: number;
  sessionCount: number;
  memoryCount: number;
  browserRunCount: number;
};

export type AppSnapshot = {
  providers: LlmProvider[];
  providerTemplates: ProviderTemplate[];
  agents: AgentProfile[];
  folders: WorkspaceFolder[];
  notes: Note[];
  fileRecords: FileRecord[];
  sessions: AgentSession[];
  memoryEntries: MemoryEntry[];
  browserRuns: BrowserRun[];
  rigMarker: string;
};

export type ChatMessageResponse = {
  sessionId: string;
  userMessage: SessionMessage;
  assistantMessage: SessionMessage;
  output: string;
};

export type AppView = "providers" | "agents" | "chat" | "files" | "memory";
