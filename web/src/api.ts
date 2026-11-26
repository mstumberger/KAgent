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

/** A compaction by hand (compaction.md section 4): everything before the last user message of the session is summarized. */
export function requestCompaction(id: string): Promise<CompactResult> {
  return api.post<CompactResult>(`/api/sessions/${id}/compact`, {});
}

// ---- shared API types (api.md section 2)

export interface SessionMeta {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  messageCount: number;
  /** From compaction (compaction.md section 4): the summary of the first `summaryUpto` stored messages. */
  summary?: string | null;
  summaryUpto?: number | null;
  /** The session's todo list (plan-todo.md section 1); empty while there is none. */
  todo?: TodoItem[];
  /** Plan mode (plan-todo.md section 2): on while the session plans; `approvedPlan` is the plan the owner approved last. */
  planning?: boolean;
  approvedPlan?: { path: string; text: string } | null;
}

/** One item of a session's todo list (api.md section 6). */
export interface TodoItem {
  content: string;
  status: "pending" | "in_progress" | "completed";
  priority: "high" | "medium" | "low";
}

/** Replace the todo list of a session; an empty list clears it. The daemon normalises what it receives and answers with the stored list. */
export function putTodo(id: string, items: TodoItem[]): Promise<{ items: TodoItem[] }> {
  return api.put(`/api/sessions/${id}/todo`, { items });
}

/** Put a session in or out of plan mode ("Cancel plan" sends `{active: false}`); the answer is its flag. */
export function postPlan(id: string, active: boolean): Promise<{ planning: boolean }> {
  return api.post(`/api/sessions/${id}/plan`, { active });
}

/** The answer of `POST /api/sessions/:id/compact`: `keptMessages` stored messages follow the summary unchanged. */
export interface CompactResult {
  summary: string;
  keptMessages: number;
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
  voice?: { brain?: VoiceBrain; toolGroups?: string[]; context?: boolean; vocabulary?: boolean; pauseMedia?: boolean };
  /** settings.browser: the DevTools port and binary of KAgent's own window (empty = detect); `backend` (S27): `own` = that window, `attach` = the owner's running browser, with `attachProfile` = its user data folder (empty: the first found) and `attachTrust` `strict` | `normal` (browser.md). */
  browser?: { port?: number; binary?: string; backend?: "own" | "attach"; attachProfile?: string; attachTrust?: "strict" | "normal" };
  /** settings.telegram (bots.md): the Telegram bot, off until the owner pairs a chat. */
  telegram?: { enabled?: boolean; notifyAlerts?: boolean; forwardApprovals?: boolean; approvalForwardSec?: number; allowPrivileged?: boolean };
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

// ---- skills (api.md section 6)

/** What POST and PUT `/api/skills` send; PUT leaves out what stays. */
export interface SkillDraft {
  name?: string;
  description?: string;
  whenToUse?: string;
  body?: string;
}

export interface SkillView {
  name: string;
  description: string;
  whenToUse?: string;
  source: "bundled" | "user" | "imported";
  /** The skill's folder; `realPath` is it with every link resolved. */
  path: string;
  realPath: string;
  enabled: boolean;
  /** A skill of the owner hides it (same name, higher row of skills.md section 2). */
  shadowed: boolean;
  sizeBytes: number;
  /** Only `get` and the answers of writes carry it; the list does not. */
  body?: string;
  uses: number;
  lastUsed?: string;
  symlink: boolean;
  /** When `SKILL.md` was last written, RFC 3339. */
  updatedAt?: string;
  /** An imported skill: where it came from (S21). */
  importedFrom?: { source: string; path: string; hash: string };
}

/** A skill of another tool's folder, as the import dialog lists it (S21). */
export interface ImportCandidate {
  name: string;
  description: string;
  /** The skill's folder; `realPath` is it with links resolved. */
  path: string;
  realPath: string;
  /** A skill has the same folder or the same name. */
  installed: boolean;
  /** The name of that skill. */
  duplicateOf?: string;
}

/** A folder of another tool that was looked at; `exists` is false when the tool is not installed. */
export interface ImportSource {
  agent: string;
  path: string;
  exists: boolean;
  skills: ImportCandidate[];
}

export type ImportMode = "copy" | "symlink";

/** Why a skill was not imported or installed. */
export interface SkippedSkill {
  name: string;
  reason: string;
}

/** What an import did. `disabled` names the skills that arrived switched off. */
export interface ImportResult {
  imported: string[];
  skipped: SkippedSkill[];
  disabled: string[];
}

/** A git source (api.md `SourceView`). */
export interface SourceView {
  id: string;
  name: string;
  /** An https:// address, or the path of a folder. */
  url: string;
  /** Branch, tag or commit; empty is the default branch. */
  ref: string;
  /** The folder inside the repository that holds the skills; empty is its root. */
  subdir: string;
  commit?: string;
  lastSync?: string;
  lastError?: string;
  skills: { name: string; hash: string; syncedAt: string }[];
}

/** What adding a source (its first install), or syncing one, did. */
export interface SourceReport {
  added: string[];
  updated?: string[];
  removed?: string[];
  skipped: SkippedSkill[];
}

/** What `POST /api/skills/sources` sends. */
export interface SourceForm {
  url: string;
  ref?: string;
  subdir?: string;
  name?: string;
}

/** One suggestion of the catalog. */
export interface CatalogEntry {
  name: string;
  url: string;
  subdir: string;
  note: string;
}

export const skillsApi = {
  /** The library. A daemon without the route (before S20) gives an empty list. */
  list: async (): Promise<SkillView[]> => {
    const r = await api.get<{ skills?: SkillView[] }>("/api/skills");
    return Array.isArray(r?.skills) ? r.skills : [];
  },
  get: (name: string) => api.get<SkillView>(`/api/skills/${encodeURIComponent(name)}`),
  create: (draft: SkillDraft) => api.post<SkillView>("/api/skills", draft),
  update: (name: string, draft: SkillDraft) => api.put<SkillView>(`/api/skills/${encodeURIComponent(name)}`, draft),
  remove: (name: string) => api.del(`/api/skills/${encodeURIComponent(name)}`),
  setEnabled: (name: string, enabled: boolean) => api.put<SkillView>(`/api/skills/${encodeURIComponent(name)}/enabled`, { enabled }),
  rescan: () => api.post<{ count: number }>("/api/skills/rescan", {}),
  // ---- import and sources (S21)
  /** The folders of other tools under the home directory. */
  importSources: async (): Promise<ImportSource[]> => {
    const r = await api.get<{ sources?: ImportSource[] }>("/api/skills/import-sources");
    return Array.isArray(r?.sources) ? r.sources : [];
  },
  import: (path: string, names: string[], mode: ImportMode) => api.post<ImportResult>("/api/skills/import", { path, names, mode }),
  sources: async (): Promise<{ sources: SourceView[]; catalog: CatalogEntry[] }> => {
    const r = await api.get<{ sources?: SourceView[]; catalog?: CatalogEntry[] }>("/api/skills/sources");
    return { sources: Array.isArray(r?.sources) ? r.sources : [], catalog: Array.isArray(r?.catalog) ? r.catalog : [] };
  },
  /** Clones (an address) or reads a folder, and installs the skills switched off. */
  addSource: (form: SourceForm) => api.post<SourceView & SourceReport>("/api/skills/sources", form),
  syncSource: (id: string) => api.post<SourceReport>(`/api/skills/sources/${encodeURIComponent(id)}/sync`, {}),
  removeSource: (id: string, removeSkills: boolean) => api.del(`/api/skills/sources/${encodeURIComponent(id)}`, { removeSkills }),
};

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
