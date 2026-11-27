// API client: token from #token=... (stored once, hash cleared) -> localStorage.

const BASE = "";
const TOKEN_KEY = "kagent.token";

/** Session to open from the URL (`#token=..&session=..`, KRunner "Open in KAgent"). */
export let initialSession: string | null = null;
/** View to open from the URL (`#view=alerts`). */
export let initialView: string | null = null;
/** Settings page to open from the URL (`#view=settings&section=tools`). */
export let initialSection: string | null = null;

export function captureToken(): string | null {
  const m = window.location.hash.match(/token=([0-9a-f]+)/);
  const sess = window.location.hash.match(/session=([0-9a-f-]+)/);
  if (sess) initialSession = sess[1];
  const view = window.location.hash.match(/view=([a-z]+)/);
  if (view) initialView = view[1];
  const section = window.location.hash.match(/section=([a-z]+)/);
  if (section) initialSection = section[1];
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
  /** `DELETE`, with a body where a route reads one (`{removeSkills}` of a source). */
  del: <T>(path: string, body?: unknown) => request<T>("DELETE", path, body),
  wsUrl(): string {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const token = localStorage.getItem(TOKEN_KEY) ?? "";
    return `${proto}://${location.host}/ws?token=${token}`;
  },
};

// ---- shared API types (api.md section 2); the session ones are in sessionApi.ts
export * from "./sessionApi";

export interface Message {
  role: "user" | "assistant" | "tool";
  content: string;
  toolCalls?: { id: string; name: string; input: unknown }[];
  toolCallId?: string;
  toolName?: string;
  isError?: boolean;
  via?: "text" | "voice";
}

/** What the built-in rules give a model, and what is sent with a request: the config of config.md section 3, resolved (models.md section 4). */
export interface ResolvedModelConfig {
  enabled: boolean;
  contextWindow: number;
  maxOutputTokens: number;
  supportsToolCall: boolean;
  supportsImage: boolean;
  reasoningLevels: string[];
  /** A level and the body patch sent for it. */
  reasoningPatches: Record<string, unknown>;
  supportsLive: boolean;
  liveProtocol: string | null;
  requestPatch: unknown;
  roles: string[];
}

/** The personal patch of a model: what the owner changed. The dialog edits these six fields (models.md section 1). */
export interface ModelConfigPatch {
  contextWindow?: number;
  maxOutputTokens?: number;
  supportsToolCall?: boolean;
  supportsImage?: boolean;
  reasoningLevels?: string[];
  reasoningPatches?: Record<string, unknown>;
}

/** One row of a provider in `GET /api/providers`. `inherited` is what the rules give it, `effective` what is sent. */
export interface ModelRow {
  id: string;
  name: string;
  enabled: boolean;
  /** `chat`, `live`, or empty: no job. */
  roles: string[];
  /** On: `config` holds only the overrides. Off: its six fields are the model's settings. */
  smart: boolean;
  config: ModelConfigPatch;
  inherited: ResolvedModelConfig;
  effective: ResolvedModelConfig;
}

/** What `POST /api/providers` sends: a template with its key, or `custom` with a name, an address and a format. */
export interface NewProvider {
  templateId: string;
  apiKey?: string;
  name?: string;
  baseUrl?: string;
  apiType?: string;
}

export interface ProviderView {
  id: string;
  name: string;
  templateId: string;
  enabled: boolean;
  api: { type: string; baseUrl: string; headers: Record<string, string> };
  apiKeyMasked: string;
  hasKey: boolean;
  models: ModelRow[];
  modelsFetchedAt?: string | null;
  lastError: string;
}

/** One model of a list of `GET /api/models` (`chat`, `fast`, `live`, `vision`): enabled, with the role of its list. */
export interface ModelView {
  id: string;
  name: string;
  /** `<providerId>::<modelId>`. */
  ref: string;
  config: ResolvedModelConfig;
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

/** `routed`: the chat model thinks and the live model reads its reply; `live`: the live model alone; `auto`: the live model decides per utterance. */
export type VoiceBrain = "routed" | "live" | "auto";

export interface Settings {
  chatModel: string;
  fastModel?: string;
  liveModel?: string;
  liveVoice?: string;
  fallbackModel?: string;
  visionModel?: string;
  permissionMode: "read-only" | "ask" | "trusted" | "full";
  rules: { tool: string; prefix: string }[];
  /** Tool name -> `allow`, `ask` or `deny`; applied after the mode, before the rules. */
  toolOverrides?: Record<string, string>;
  toolGroups: string[];
  /** The groups other agents may use over MCP. */
  mcpToolGroups?: string[];
  privilege?: "sudo" | "pkexec" | "none";
  maxIterations: number;
  /** Seconds a tool may run before its approval card counts as refused. */
  approvalTimeoutSec?: number;
  options: { reasoningLevel?: string | null; maxOutputTokens?: number | null; temperature?: number | null };
  persona: { name: string; address: string; language: string };
  startup?: { autostart: boolean; greeting: { enabled: boolean; text: string } };
  /** Who thinks in a voice session (voice.md section 1). */
  voice?: { brain?: VoiceBrain; toolGroups?: string[]; context?: boolean; vocabulary?: boolean; pauseMedia?: boolean; volume?: number };
  /** settings.browser: the DevTools port and binary of KAgent's own window (empty = detect); `backend` (S27): `own` = that window, `attach` = the owner's running browser, with `attachProfile` = its user data folder (empty: the first found) and `attachTrust` `strict` | `normal` (browser.md). */
  browser?: { port?: number; binary?: string; backend?: "own" | "attach"; attachProfile?: string; attachTrust?: "strict" | "normal" };
  /** settings.telegram (bots.md): the Telegram bot, off until the owner pairs a chat. */
  telegram?: { enabled?: boolean; notifyAlerts?: boolean; forwardApprovals?: boolean; approvalForwardSec?: number; allowPrivileged?: boolean };
  /** settings.calendar (calendar.md): Google Calendar and the morning report. The client secret, the token and the Maps key are not here. */
  calendar?: {
    enabled?: boolean;
    clientId?: string;
    calendars?: string[];
    defaultCalendar?: string;
    defaultDurationMin?: number;
    home?: string;
    travelMarginMin?: number;
    report?: { enabled?: boolean; weekdays?: string; weekends?: string; spoken?: boolean; telegram?: boolean; tomorrow?: boolean; tasks?: boolean; leaveBy?: boolean };
  };
  desktop?: { editor?: string; projectRoots?: string[] };
  watchers?: { quietHours?: string };
  /** Open the small widget when the daemon starts (default true). */
  startWidget?: boolean;
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
  mcp?: boolean;
}

export interface ApprovalRequest {
  /** `plan` for a plan to approve (its text is `preview`); left out by a daemon before S24, and then a tool call. */
  kind?: "tool" | "plan";
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
  /** thinkMedianMs: the median of the last 5 brain turns of a voice session (null: none yet). */
  voice?: { thinkMedianMs: number | null };
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
