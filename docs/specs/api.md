# Spec: daemon API (HTTP + WebSocket)

Base: `http://127.0.0.1:7878`. All JSON is camelCase. Types referenced here are defined
in `config.md` (settings, providers) and `crates/kagent-core/src/protocol.rs`
(Message, AgentEvent).

## 1. Transport and auth

- Bind `127.0.0.1:7878` only. Reject any request whose `Host` header is not
  `127.0.0.1:7878` or `localhost:7878` with 421.
- Every `/api/*` request needs `Authorization: Bearer <token>`; WebSockets accept
  `?token=` instead. Missing/wrong -> 401. Compare in constant time.
- No CORS headers (same-origin only). The web app is served from `/` (static
  `web/dist`, SPA fallback to `index.html`) without auth; it reads the token from
  `location.hash` (`#token=...`) once, stores it in localStorage, clears the hash.
- Errors: HTTP status + `{"error": {"code": "not_found|invalid_input|conflict|unauthorized|provider_error|internal", "message": "..."}}`.
- Request body limit 2 MiB.

## 2. Shared types

```ts
type SessionMeta = { id: string; title: string; createdAt: string; updatedAt: string; messageCount: number; running: boolean };
type ProviderView = { id: string; name: string; templateId: string; enabled: boolean;
  api: { type: ApiType; baseUrl: string; headers: Record<string,string> };
  apiKeyMasked: string; hasKey: boolean; models: ModelView[]; modelsFetchedAt: string; lastError: string };
type ModelView = { id: string; name: string; enabled: boolean; ref: string; // "<providerId>::<modelId>"
  config: ResolvedModelConfig };   // config.md ModelConfig, fully resolved
type Template = { id: string; name: string; api: {type: ApiType; baseUrl: string}; keyUrl: string; models: string[]; liveModels: string[]; voices: string[] };
type ToolView = ToolContract & { group: string; enabled: boolean };  // tool.rs ToolContract
type Change = /* config.md section 7 */;
type Alert = /* alerts.md section 1 */;
type AuditRecord = /* config.md section 6 */;
```

## 3. Endpoints

| Method path | Request | Response |
|---|---|---|
| GET `/api/state` | | `{version, settings, pendingApprovals: ApprovalRequest[], running: string[] /*session ids*/, chatModelValid: boolean, privHelper: "installed"\|"missing", alertsOpen: number}` |
| PUT `/api/settings` | partial settings | full settings. Changing `permissionMode`/`rules` applies immediately to the engine |
| GET `/api/providers/templates` | | `Template[]` |
| GET `/api/providers` | | `ProviderView[]` in `order` |
| POST `/api/providers` | `{templateId, name?, baseUrl?, apiKey?, headers?}` | `ProviderView` (201). Prefills models from the template |
| PUT `/api/providers/:id` | `{name?, baseUrl?, apiKey? /*"" = keep, null = clear*/, headers?, enabled?, models?: [{id, enabled, config?}]}` | `ProviderView` |
| DELETE `/api/providers/:id` | | 204; if it held `chatModel`, settings.chatModel = "" |
| POST `/api/providers/:id/fetch-models` | | `ProviderView` (merged; new models disabled unless < 5 total) or 502 `provider_error` |
| POST `/api/providers/:id/test` | `{modelId?}` | `{ok, latencyMs, error?}` (one short completion "Reply with OK") |
| PUT `/api/providers/order` | `{order: string[]}` | 204 |
| GET `/api/models` | | `{chat: ModelView[], fast: ModelView[], live: ModelView[], vision: ModelView[]}` (enabled only) |
| GET `/api/tools` | | `ToolView[]` |
| POST `/api/tools/:name/call` | `{input, sessionId?: string /*default "mcp"*/}` | `{status: ToolCallStatus, output: string, durationMs}` (runs `Agent::execute_call`, approvals included; used by MCP) |
| GET `/api/sessions` | | `SessionMeta[]` newest first |
| POST `/api/sessions` | `{title?}` | `SessionMeta` (201) |
| GET `/api/sessions/:id` | | `{meta: SessionMeta, messages: Message[]}` |
| DELETE `/api/sessions/:id` | | 204 (cancels if running) |
| POST `/api/sessions/:id/messages` | `{text, via?: "text"\|"voice"}` | `{status: "started", turnId}` or `{status: "queued"}` |
| POST `/api/sessions/:id/cancel` | | `{cancelled: boolean}` |
| POST `/api/stop-all` | | `{cancelledTurns: number, killedProcesses: number}` |
| POST `/api/approvals/:requestId` | `{decision: "allowOnce"\|"allowSession"\|"allowAlways"\|"deny", reason?}` | 204, or 404 if already resolved |
| GET `/api/system` | | `system_info` JSON |
| GET `/api/voice/levels?since=<seq>` | | `{seq, state: "speaking"\|"listening"\|"thinking"\|"tool"\|"approval"\|"idle", detail, bands: 16, frames: [{seq, tMs, who: "user"\|"assistant", rms, bands: number[16]}]}` (max 96 frames, newest; 0..1 loudness envelope) |
| GET `/api/doctor` | | `{score, checks: [{name, status: ok\|warn\|bad, detail, penalty}], at}` |
| GET `/api/sentinel` | | `{honeytokens: [{path, kind, marker}], baseline: bool}` |
| GET `/api/net` | | `{configured, config: RouterConfig, router: {ok, resource?, connections?, interfaces?, error?}}` |
| PUT `/api/net/router` | `{host, user, password?, flowPort, honeypotIp, trapUser, https, fingerprint?}` | same as GET; pins the TLS fingerprint on first contact, persists `settings.mikrotik`, restarts collectors |
| GET `/api/net/devices` / `/api/net/top?device=&minutes=` | | device rows / top flows |
| POST `/api/net/unblock` | `{address}` | `{removed}` (undo path of router_block_ip) |
| POST `/api/voice/demo` | | 202; plays ~6 s of synthetic speech levels (test visualizers without voice) |
| GET `/api/audit?limit=100&before=seq` | | `AuditRecord[]` |
| GET `/api/audit/verify` | | `{ok: true, count}` or `{ok: false, brokenAt: seq, reason}` |
| GET `/api/changes?limit=50` | | `Change[]` |
| POST `/api/changes/:id/undo` | | `Change` (status updated); goes through approval like a tool call (undo_change) |
| GET `/api/alerts?status=open` | | `Alert[]` |
| POST `/api/alerts/:id/ack` | `{note?}` | `Alert` |
| POST `/api/alerts/:id/investigate` | | `{sessionId}`: new session seeded with the alert evidence, turn started |
| POST `/api/sentinel/honeytokens` | `{enable: boolean}` | `{tokens: [{path, kind}]}` |
| POST `/api/sentinel/rebaseline` | | `{takenAt, items: number}` |
| GET `/api/net/devices` | | devices rows (config.md 10) |
| GET `/api/net/top?device=&minutes=60` | | `[{dst, dport, proto, bytes}]` |

Note: `allowSession` is an addition to the drafted `ApprovalDecision` enum in
`permission.rs` (S4 adds it): allowed for the rest of this daemon run, per tool +
rule_key, kept in memory only.

## 4. WebSocket `/ws`

Server -> client only. Each message is one `AgentEvent` (protocol.rs, `type` tag):
`turnStarted, textDelta, reasoningDelta, messageAppended, toolCallStarted,
toolCallFinished, approvalRequested, approvalResolved, turnFinished, sessionUpdated,
notice`, plus added variants:

```json
{"type": "alertRaised", "alert": {...Alert}}
{"type": "alertUpdated", "alert": {...Alert}}
{"type": "changeRecorded", "change": {...Change}}
{"type": "settingsChanged", "settings": {...}}
{"type": "liveStatus", "sessionId": "...", "status": "connecting|live|rolling|idle|error", "detail": ""}
```
Note: `GET /api/providers` returns `{providers: [...]}` and `GET /api/tools` returns
`{tools: [{contract, group, enabled, mcp}]}` (wrapped objects, as implemented).

D-Bus (KRunner): service `org.kagent.KRunner`, path `/runner`, interface
`org.kde.krunner1` (`Match`, `Run`, `Actions`); queries prefixed `ka ` / `kagent `.
```
```
- On connect the server sends `{"type":"hello","version":"x.y.z","pendingApprovals":[...]}`.
- Lagging client (broadcast `Lagged`): send `notice` level `warn` "events dropped,
  reload session" and continue.
- Ping every 20 s; close after 60 s without pong.

## 5. WebSocket `/live?session=<id>` (S10)

Client -> server:
```json
{"type": "start", "voice": "Kore"}                 // optional voice override
{"type": "audio", "data": "<base64 PCM16 LE mono 16 kHz>"}   // 20-100 ms chunks
{"type": "text", "text": "..."}                    // typed input into the live session
{"type": "stop"}
```
Server -> client:
```json
{"type": "status", "status": "connecting|live|rolling|idle|error", "detail": ""}
{"type": "audio", "data": "<base64 PCM16 LE mono>", "rate": 24000}
{"type": "transcript", "who": "user|assistant", "text": "...", "final": false}
{"type": "interrupted"}
{"type": "tool", "callId": "...", "name": "...", "state": "started|waitingApproval|done|error|denied"}
{"type": "turnComplete"}
```
- One live socket per session; a second one gets `status:error detail:"busy"`.
- No live model configured -> `status:error detail:"no live model"` then close.

## 6. Phase 4 endpoints (S17-S33)

Same transport, auth and error shape as section 3. `GET /api/sessions` without
`parent` hides sessions with origin `subagent`. `GET /api/sessions/:id` returns the
phase 4 meta fields of config.md section 11 (`origin`, `parent`, `summary`,
`todo`, `planning`, `approvedPlan`). `ApprovalRequest` gains `kind: "tool"|"plan"`;
for a plan, `preview` holds the plan text.

| Area | Method path | Request | Response |
|---|---|---|---|
| skills | GET `/api/skills` | | `{skills: SkillView[]}` (enabled and disabled) |
| | GET `/api/skills/:name` | | `SkillView` with `body` |
| | POST `/api/skills` | `{name, description, whenToUse?, body}` | `SkillView` (201), user skill |
| | PUT `/api/skills/:name` | `{description?, whenToUse?, body?}` | `SkillView`; user skills only, else 409 |
| | DELETE `/api/skills/:name` | | 204; user and imported skills, bundled -> 409 |
| | PUT `/api/skills/:name/enabled` | `{enabled: boolean}` | `SkillView` |
| | POST `/api/skills/rescan` | | `{count}` |
| | GET `/api/skills/import-sources` | | `{sources: [{agent, path, exists, skills: [{name, description, path, realPath, installed, duplicateOf?}]}]}` |
| | POST `/api/skills/import` | `{path, names: string[], mode: "copy"\|"symlink"}` | `{imported: string[], skipped: [{name, reason}]}` |
| | GET `/api/skills/sources` | | `{sources: SourceView[], catalog: [{name, url, subdir, note}]}` |
| | POST `/api/skills/sources` | `{url, ref?, subdir?, name?}` | `SourceView` (clones and installs) |
| | POST `/api/skills/sources/:id/sync` | | `{added: string[], updated: string[], removed: string[]}` |
| | DELETE `/api/skills/sources/:id` | `{removeSkills?: boolean}` | 204 |
| memory | GET `/api/memory` | | `{enabled, core: MemoryView[], memories: MemoryView[], stats: {files, bytes, unreviewed}}` |
| | GET `/api/memory/search?q=` | | `MemoryView[]` |
| | POST `/api/memory` | `{name, description?, type, content}` | `MemoryView` (201) |
| | PUT `/api/memory/:name` | `{description?, type?, content?, reviewed?}` | `MemoryView` |
| | DELETE `/api/memory/:name` | | 204; core files -> 409 |
| compaction | POST `/api/sessions/:id/compact` | | `{summary: string, keptMessages: number}` |
| todo, plan | PUT `/api/sessions/:id/todo` | `{items: TodoItem[]}` | `{items: TodoItem[]}` |
| | POST `/api/sessions/:id/plan` | `{active: false}` | `{planning: boolean}` |
| | POST `/api/sessions/:id/messages` | `{text, via?, plan?: boolean}` (S24) | as section 3; `plan: true` or a text starting with `/plan ` starts the turn in plan mode |
| browser | GET `/api/browser/status` | | `{backend, own: {running, port, tabs}, attach: {profile, browser, port, debugging: "on"\|"off"\|"unknown", connected, tabs, detail}}` |
| | POST `/api/browser/attach/connect` | | same as status, after one connect attempt (waits up to 20 s for Allow) |
| | POST `/api/browser/attach/disconnect` | | 204 |
| tasks | GET `/api/tasks`, POST `/api/tasks` | | `{tasks: Task[]}`; `automations.md` (existing routes, specified there) |
| | PUT, DELETE `/api/tasks/:id` | partial `Task` | `{tasks: Task[]}` |
| | POST `/api/tasks/:id/run` | | `{tasks: Task[]}` |
| | GET `/api/tasks/:id/runs` | | `{runs: TaskRun[]}` |
| | POST `/api/tasks/:id/cancel` | | `{cancelled: boolean}` |
| telegram | GET `/api/telegram` | | `{enabled, configured, botUsername, paired, chatId, pairedAt, lastError, pendingCode}` |
| | PUT `/api/telegram` | `{enabled?, notifyAlerts?, forwardApprovals?, approvalForwardSec?, allowPrivileged?}` | same as GET |
| | POST `/api/telegram/token` | `{token}` | `{botUsername}`: stores the token in Secret Service, calls `getMe` |
| | DELETE `/api/telegram/token` | | 204; also unpairs |
| | POST `/api/telegram/pairing-code` | | `{code, expiresAt}` |
| | DELETE `/api/telegram/pairing` | | 204 |
| | POST `/api/telegram/test` | | `{ok, error?}` |
| subagents | GET `/api/agents` | | `{agents: AgentProfileView[]}` |
| | PUT `/api/agents/:name` | `{description, model?, toolGroups?, maxIterations?, prompt}` | `AgentProfileView`: creates or updates; built-in names -> 409 |
| | DELETE `/api/agents/:name` | | 204 |
| | GET `/api/sessions?parent=<id>` | | `SessionMeta[]` |
| hooks | GET `/api/hooks` | | `{hooks: HookView[], events: string[]}` |
| | POST `/api/hooks` | `Hook` without `id` | `HookView` (201) |
| | PUT `/api/hooks/:id` | `Hook` | `HookView` |
| | DELETE `/api/hooks/:id` | | 204 |
| | POST `/api/hooks/:id/test` | `{event, sample?: object}` | `{exitCode, stdout, stderr, decision, durationMs, timedOut}` |
| widgets | GET `/api/widgets/:session/:id` | | `text/html` (bearer auth) |

```ts
type SkillView = { name: string; description: string; whenToUse?: string;
  source: "bundled" | "user" | "imported"; path: string; realPath: string; enabled: boolean;
  shadowed: boolean; sizeBytes: number; body?: string; uses: number; lastUsed?: string;
  symlink: boolean; importedFrom?: { source: string; path: string; hash: string } };
type SourceView = { id: string; name: string; url: string; ref: string; subdir: string;
  commit?: string; lastSync?: string; lastError?: string; skills: [{ name: string; hash: string; syncedAt: string }] };
type MemoryView = { name: string; description: string; type: string; path: string; bytes: number;
  createdAt: string; updatedAt: string; source: "user" | "agent"; reviewed: boolean;
  validUntil?: string; supersededBy?: string; content?: string };
type TodoItem = { content: string; status: "pending" | "in_progress" | "completed"; priority: "high" | "medium" | "low" };
type TaskRun = { at: string; status: "ok" | "failed" | "cancelled" | "timeout" | "missed"; durationSec?: number;
  sessionId?: string; result?: string };
type AgentProfileView = { name: string; description: string; builtIn: boolean; source: "builtin" | "user";
  model: string; toolGroups: string[]; maxIterations: number; prompt: string };
type HookView = /* config.md section 11, Hook */;
```

## 7. Events added in phase 4 (`/ws`)

```json
{"type":"skillsChanged","count":12}
{"type":"memoryChanged","name":"router","removed":false}
{"type":"todoUpdated","sessionId":"...","items":[{"content":"...","status":"pending","priority":"medium"}]}
{"type":"planUpdated","sessionId":"...","active":true}
{"type":"tasksChanged"}
{"type":"taskRun","taskId":"...","run":{"at":"...","status":"ok","durationSec":42,"sessionId":"...","result":"..."}}
{"type":"subagentUpdated","sessionId":"...","child":{"id":"...","title":"..."},"status":"running|done|failed|cancelled"}
{"type":"telegramStatus","status":"off|disabled|unpaired|polling|error","detail":""}
```

`uiCommand` `show` args gain `section` (settings section id). `notice` with level
`tasks` is no longer used once S28 is done (`tasksChanged` replaces it).
