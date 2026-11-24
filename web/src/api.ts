// API client: token from #token=... (stored once, hash cleared) -> localStorage.

const BASE = "";
const TOKEN_KEY = "kagent.token";

/** Session to open from the URL (`#token=..&session=..`, KRunner "Open in KAgent"). */
export let initialSession: string | null = null;
/** View to open from the URL (`#view=alerts`). */
export let initialView: string | null = null;

export function captureToken(): string | null {
  const m = window.location.hash.match(/token=([0-9a-f]+)/);
  const sess = window.location.hash.match(/session=([0-9a-f-]+)/);
  if (sess) initialSession = sess[1];
  const view = window.location.hash.match(/view=([a-z]+)/);
  if (view) initialView = view[1];
  if (m) {
    localStorage.setItem(TOKEN_KEY, m[1]);
    history.replaceState(null, "", window.location.pathname + window.location.search);
    return m[1];
  }
  return localStorage.getItem(TOKEN_KEY);
}

export class ApiError extends Error {
  status: number;
  code: string;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const token = localStorage.getItem(TOKEN_KEY) ?? "";
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${token}`,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (res.status === 204) return undefined as T;
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    const err = data?.error ?? {};
    throw new ApiError(res.status, err.code ?? "unknown", err.message ?? res.statusText);
  }
  return data as T;
}

export const api = {
  get: <T>(path: string) => request<T>("GET", path),
  put: <T>(path: string, body: unknown) => request<T>("PUT", path, body),
  post: <T>(path: string, body?: unknown) => request<T>("POST", path, body),
  del: <T>(path: string) => request<T>("DELETE", path),
  wsUrl(): string {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const token = localStorage.getItem(TOKEN_KEY) ?? "";
    return `${proto}://${location.host}/ws?token=${token}`;
  },
};

// ---- shared API types (api.md section 2)

export interface SessionMeta {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  messageCount: number;
}

export interface Message {
  role: "user" | "assistant" | "tool";
  content: string;
  toolCalls?: { id: string; name: string; input: unknown }[];
  toolCallId?: string;
  toolName?: string;
  isError?: boolean;
  via?: "text" | "voice";
}

export interface Session {
  meta: SessionMeta;
  messages: Message[];
}

export interface ProviderView {
  id: string;
  name: string;
  templateId: string;
  enabled: boolean;
  api: { type: string; baseUrl: string; headers: Record<string, string> };
  apiKeyMasked: string;
  hasKey: boolean;
  models: { id: string; name: string; enabled: boolean; roles: string[] }[];
  modelsFetchedAt?: string | null;
  lastError: string;
}

export interface Template {
  id: string;
  name: string;
  api: { type: string; baseUrl: string };
  keyUrl: string;
  models: string[];
  liveModels: string[];
  voices: string[];
}

export interface Settings {
  chatModel: string;
  fastModel?: string;
  liveModel?: string;
  permissionMode: "read-only" | "ask" | "trusted";
  rules: { tool: string; prefix: string }[];
  toolGroups: string[];
  maxIterations: number;
  persona: { name: string; address: string; language: string };
  startup?: { autostart: boolean; greeting: { enabled: boolean; text: string } };
  [key: string]: unknown;
}

export interface ToolView {
  contract: {
    name: string;
    description: string;
    sideEffect: string;
    readOnly: boolean;
    destructive: boolean;
    privileged: boolean;
    risk: string;
  };
  group: string;
  enabled: boolean;
}

export interface ApprovalRequest {
  preview?: string;
  requestId: string;
  sessionId: string;
  tool: string;
  input: unknown;
  summary: string;
  reason: string;
  risk: string;
  privileged: boolean;
  canRemember: boolean;
  ruleKey: string;
}

export interface StateView {
  version: string;
  settings: Settings;
  pendingApprovals: ApprovalRequest[];
  running: string[];
  chatModelValid: boolean;
  privHelper: string;
  alertsOpen: number;
}

export interface Alert {
  id: string;
  ts: string;
  lastSeen: string;
  count: number;
  rule: string;
  severity: "info" | "review" | "high";
  status: "open" | "acked" | "resolved";
  title: string;
  cause: string;
  evidence: { kind: string; text: string }[];
  proposal: string;
  note?: string;
}

export interface Change {
  id: string;
  ts: string;
  tool: string;
  summary: string;
  undo: unknown | null;
  privileged: boolean;
  status: "applied" | "failed" | "undone" | "undo_failed";
  before?: string;
  after?: string;
  note?: string;
}
