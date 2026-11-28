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
| GET `/api/state` | | `{version, settings, pendingApprovals: ApprovalRequest[], running: string[] /*session ids*/, chatModelValid: boolean, privHelper: "installed"\|"missing", alertsOpen: number, voice: {thinkMedianMs: number\|null}}` (`voice.thinkMedianMs`: the median of the last 5 brain turns of a voice session, in ms, kept in memory; `null` until one ran; Settings -> Voice warns over 4000) |
| PUT `/api/settings` | partial settings | full settings. Changing `permissionMode`/`rules` applies immediately to the engine |
| GET `/api/providers/templates` | | `Template[]` |
| GET `/api/providers` | | `ProviderView[]` in `order` |
| POST `/api/providers` | `{templateId, name?, baseUrl?, apiKey?, headers?}` | `ProviderView` (201). Prefills models from the template |
| PUT `/api/providers/:id` | `{name?, baseUrl?, apiKey? /*"" = keep, null = clear*/, headers?, enabled?, models?: [{id, enabled, config?}]}` | `ProviderView` |
| DELETE `/api/providers/:id` | | 204; every model setting naming one of its models (`chatModel`, `fastModel`, `fallbackModel`, `visionModel`, `liveModel`) is set to "" |
| POST `/api/providers/:id/fetch-models` | | `ProviderView` (merged into the entry as it is when the answer arrives; new models arrive disabled, models.md section 3) or 502 `provider_error` |
| POST `/api/providers/:id/test` | `{modelId?}` | `{ok, latencyMs, error?}` (one short completion "Reply with OK") |
| PUT `/api/providers/order` | `{order: string[]}` | 204 |
| GET `/api/models` | | `{chat: ModelView[], fast: ModelView[], live: ModelView[], vision: ModelView[]}` (enabled only) |
| GET `/api/tools` | | `ToolView[]` |
| POST `/api/tools/:name/call` | `{input, sessionId?: string /*default "mcp"*/}` | `{status: ToolCallStatus, output: string, durationMs}` (runs `Agent::execute_call`, approvals included; used by MCP) |
| GET `/api/sessions` | | `SessionMeta[]` newest first |
| POST `/api/sessions` | `{title?, model?, reasoningLevel?}` (the last two from S41, section 8) | `SessionMeta` (201) |
| GET `/api/sessions/:id` | | `{meta: SessionMeta, messages: Message[]}` |
| DELETE `/api/sessions/:id` | | 204 (cancels if running) |
| POST `/api/sessions/:id/messages` | `{text, via?: "text"\|"voice", images?: [imageId]}` | `{status: "started", turnId}` or `{status: "queued"}`; 409 for the session of a subagent (read-only); `text` may be empty when `images` is not (S46); 400 over 4 images, 404 for an image the session does not have; 409 `refused` (with a notice) when nobody can read the images (`vision.md` section 2) |
| POST `/api/sessions/:id/images` | multipart, the file in the field `file` | `ImageRef` `{id, mime: "image/png"\|"image/jpeg", width, height, bytes}` (S46, `vision.md` section 1): re-encoded, longest side 1568, EXIF dropped; 413 over 8 MB, 415 for a type that is not PNG, JPEG or WebP, 400 for a file that does not decode, 404 unknown session |
| GET `/api/sessions/:id/images/:imageId` | | the stored image (`Cache-Control: private`, bearer token); 404 |
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
`permission.rs` (S4 adds it): allowed for the rest of this daemon run, in this session
only (per tool + rule_key, kept in memory only). A call made from another session, or
with no session id, never matches it.

## 4. WebSocket `/ws`

Server -> client only. Each message is one `AgentEvent` (protocol.rs, `type` tag):
`turnStarted, textDelta, reasoningDelta, messageAppended, toolCallStarted,
toolCallFinished, approvalRequested, approvalResolved, turnFinished, sessionUpdated,
notice`, plus added variants:

- A model answer with neither text nor a tool call is not stored; the request is sent
  once more, and a second empty answer ends the turn with `turnFinished` `status:
  "failed"`, `error: "the model gave no answer (asked twice): try again, or choose
  another chat model"` (`agent::NO_ANSWER`, v0.0.2).

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
- The `tool` event also names `think` (states `started`, `ok`, `error`, `cancelled`),
  and a `status` carries `detail: "no chat model: the live model thinks"` when
  `routed` falls back to `live` (voice.md).

## 6. Phase 4 endpoints (S17-S33), and the coding agents (S35-S38)

Same transport, auth and error shape as section 3. `GET /api/sessions` without
`parent` hides sessions with origin `subagent`. Each listed session has `agents`, how
many subagents it started. `GET /api/sessions/:id` returns the
phase 4 meta fields of config.md section 11 (`origin`, `parent`, `summary`,
`todo`, `planning`, `approvedPlan`). `ApprovalRequest` gains `kind: "tool"|"plan"`;
for a plan, `preview` holds the plan text.

| Area | Method path | Request | Response |
|---|---|---|---|
| skills | GET `/api/skills` | | `{skills: SkillView[]}` (enabled and disabled) |
| | GET `/api/skills/:name` | | `SkillView` with `body` |
| | POST `/api/skills` | `{name, description, whenToUse?, body}` | `SkillView` (201), user skill; a name the owner has already: 409 |
| | PUT `/api/skills/:name` | `{description?, whenToUse?, body?}` | `SkillView`; user skills only, else 409. A field left out keeps its value, and the file keeps every other line of its frontmatter (other keys, comments) |
| | DELETE `/api/skills/:name` | | 204; user and imported skills, bundled -> 409 |
| | PUT `/api/skills/:name/enabled` | `{enabled: boolean}` | `SkillView` |
| | POST `/api/skills/rescan` | | `{count}` |
| | GET `/api/skills/import-sources` | | `{sources: [{agent, path, exists, skills: [{name, description, path, realPath, installed, duplicateOf?}]}]}` |
| | POST `/api/skills/import` | `{path, names: string[], mode: "copy"\|"symlink"}` | `{imported: string[], skipped: [{name, reason}], disabled: string[]}` |
| | GET `/api/skills/sources` | | `{sources: SourceView[], catalog: [{name, url, subdir, note}]}` |
| | POST `/api/skills/sources` | `{url, ref?, subdir?, name?}` | `SourceView` plus `added: string[]` and `skipped: [{name, reason}]` (201; clones and installs) |
| | POST `/api/skills/sources/:id/sync` | | `{added: string[], updated: string[], removed: string[], skipped: [{name, reason}]}` |
| | DELETE `/api/skills/sources/:id` | `{removeSkills?: boolean}`, or `?removeSkills=true` | 204 |
| memory | GET `/api/memory` | | `{enabled, core: MemoryView[], memories: MemoryView[], stats: {files, bytes, unreviewed}}` |
| | GET `/api/memory/search?q=` | | `MemoryView[]` |
| | POST `/api/memory` | `{name, description?, type, content}` | `MemoryView` (201); 400 with the name of a core file, 409 when the name exists |
| | PUT `/api/memory/:name` | `{description?, type?, content?, reviewed?}` | `MemoryView` |
| | POST `/api/memory/:name/forget` | | `MemoryView`; marks it `validUntil` today. Core files -> 409, unknown -> 404 |
| | DELETE `/api/memory/:name` | | 204; core files -> 409 |
| calendar | GET `/api/calendar` | | `{state: off\|connecting\|connected\|reconnect\|error, account?, calendars: [{id, name, color, writable, on}], hasClientSecret, hasMapsKey, lastReport?: {at, text, sent: [telegram\|spoken]}, error?}`; the calendars are read from Google when connected (a failure is in `error`, the state is the one after the call); never a secret |
| | PUT `/api/calendar/client` | `{clientId, clientSecret?}` (an empty secret keeps the one stored) | 204; the id goes to `settings.calendar.clientId`, the secret to the keyring; 400 for an empty id or one with spaces |
| | POST `/api/calendar/connect` | | `{url}` for the page to open in the browser; 400 without a client id and secret |
| | GET `/api/calendar/oauth/callback` | `?code&state` (or `?error`) | a small HTML page. The one calendar route without the bearer token: loopback peer only (`ConnectInfo`), and a known, unused `state` younger than 10 minutes only; the Host check still applies |
| | POST `/api/calendar/disconnect` | | 204; revokes the token at Google, deletes it, keeps the settings and the client |
| | PUT `/api/calendar/maps-key` | `{key}` (`null` forgets it) | 204; 400 for a key that is not 16 to 128 printable characters |
| | POST `/api/calendar/maps-test` | | `{ok, place?: {name, address}, error?}`: one Places search for "Ljubljana", on the map and not the web |
| | POST `/api/calendar/report` | `{deliver: boolean}` | `{text, sent: [..]}`: the report for today; with `deliver` also sent to the channels that are on |
| compaction | POST `/api/sessions/:id/compact` | | `{summary: string, keptMessages: number}`; 400 under 4 stored messages or with nothing before the last user message, 404 unknown session, 502 `summary_failed` |
| todo, plan | PUT `/api/sessions/:id/todo` | `{items: TodoItem[]}` | `{items: TodoItem[]}`; 400 `invalid_input` without `items` or for a list `todo_write` refuses, 404 unknown session |
| | POST `/api/sessions/:id/plan` | `{active: boolean}` | `{planning: boolean}`; 400 without a boolean `active`, 404 unknown session |
| | POST `/api/sessions/:id/messages` | `{text, via?, plan?: boolean, images?: [imageId]}` (S24, S46) | as section 3; `plan: true` or a text starting with `/plan ` starts the turn in plan mode |
| browser | GET `/api/browser/status` | | `{backend, own: {running, port, tabs}, attach: {profile, browser, port, debugging: "on"\|"off"\|"unknown", connected, tabs, detail}}` |
| | POST `/api/browser/attach/connect` | | 200 with the status after one connect attempt (waits up to 20 s for Allow), also when it failed: `attach.detail` says why (`attach.port` is a number or null) |
| | POST `/api/browser/attach/disconnect` | | 204 |
| tasks | GET `/api/tasks`, POST `/api/tasks` | | `{tasks: Task[]}`; `automations.md` (existing routes, specified there) |
| | PUT, DELETE `/api/tasks/:id` | partial `Task` | `{tasks: Task[]}` |
| | POST `/api/tasks/:id/run` | | `{tasks: Task[]}` |
| | GET `/api/tasks/:id/runs` | | `{runs: TaskRun[]}` |
| | POST `/api/tasks/:id/cancel` | | `{cancelled: boolean}` |
| | GET `/api/tasks/templates` | | `{templates: [{id, title, what, kind, schedule, scheduleText}]}` |
| | POST `/api/tasks/templates/:id` | | `{tasks: Task[]}` (201): the task of a template, paused |
| | GET `/api/tasks/preview?schedule=` | | `{schedule, scheduleText, nextRun}`; 400 when the text does not parse |
| telegram | GET `/api/telegram` | | `{enabled, configured, status, botUsername, paired, chatId, pairedAt, lastError, pendingCode, pairingLockedUntil}`; a missing optional field is `null` |
| | PUT `/api/telegram` | `{enabled?, notifyAlerts?, forwardApprovals?, approvalForwardSec?, allowPrivileged?}` | same as GET; 400 for any other key |
| | POST `/api/telegram/token` | `{token}` | `{botUsername}`: calls `getMe`; stores the token in Secret Service only when Telegram knows it: 400 for a wrong shape or a refusal, 502 `telegram_unreachable` without an answer |
| | DELETE `/api/telegram/token` | | 204; also unpairs |
| | POST `/api/telegram/pairing-code` | | `{code, expiresAt}`; 400 with no token or while switched off, 409 when a chat is paired |
| | DELETE `/api/telegram/pairing` | | 204 |
| | POST `/api/telegram/test` | | `{ok, error?}`; 200 with `ok: false` when no chat is paired |
| web | GET `/api/web/brave-key` | | `{configured}`; never the key |
| | PUT `/api/web/brave-key` | `{key}` | `{configured: true}`; 400 for a key that is not 16 to 128 printable ASCII characters without a space |
| | DELETE `/api/web/brave-key` | | 204 |
| subagents | GET `/api/agents` | | `{agents: AgentProfileView[], invalid: [{file: string, reason: string}]}` |
| | PUT `/api/agents/:name` | `{description, model?, toolGroups?, maxIterations?, prompt}` | `AgentProfileView` (201 when it creates, 200 when it changes; a field left out keeps its value); built-in names -> 409; an invalid name, a missing description, an unknown tool group, `maxIterations` outside 1..80 or a model the providers do not know -> 400 |
| | DELETE `/api/agents/:name` | | 204 |
| | POST `/api/sessions/:id/subagents/:child/stop` | | `{status}`; 404 for a subagent of another session, 409 when it ended already |
| | GET `/api/sessions?parent=<id>` | | `SessionMeta[]` |
| hooks | GET `/api/hooks` | | `{hooks: HookView[], events: string[], samples: {<Event>: object}}` |
| | POST `/api/hooks` | Hook without `id` | `HookView` (201) |
| | PUT `/api/hooks/:id` | Hook | `HookView` |
| | DELETE `/api/hooks/:id` | | 204 |
| | POST `/api/hooks/:id/test` | `{event, sample?: object}` | `{exitCode, stdout, stderr, decision?, durationMs, timedOut, error?}` (`decision` is left out when the hook decided nothing) |
| widgets | GET `/api/widgets/:session/:id` | | `text/html; charset=utf-8`, the title percent-encoded in `X-KAgent-Title`, `Cache-Control: no-store` (bearer auth); 400 for a session id that is not plain or an id that is not a number from 1 to 200, 404 when there is no such fragment |
| coders | GET `/api/coders` | | `{tools: CoderToolView[], profiles: CoderProfileView[], invalid: [{file, reason}], running: CoderRunView[]}` (S35; the profiles of the owner and `invalid` from S36). A run is stopped through `/api/sessions/:id/subagents/:child/stop`, and `GET /api/sessions/:id` returns `coder` in the meta of a run (config.md section 11) |
| | POST `/api/coders/test` | `{tool, run?: boolean}` | `{tool, installed, path, version, ok?, seconds?, reply?, error?}`; with `run` it starts one short real job and uses the owner's subscription (`coders.md` section 6); 400 for an unknown tool |
| | PUT `/api/coders/:name` (S36) | `{description, tool?, mode?, model?, effort?, allowedTools?, args?, env?, prompt?}` | `CoderProfileView` (201 when it creates, 200 when it changes; a field left out keeps its value); a built-in name -> 409; an invalid name, a missing description, an unknown tool or mode, a reserved flag in `args`, a reserved name in `env` -> 400 |
| | DELETE `/api/coders/:name` (S36) | | 204; a built-in -> 409 |
| | POST `/api/coders/terminal` (S37) | `{tool, sessionId, project?}` | `{ok: true}`; 400 for an unknown tool or when no terminal can be opened; 409 while a run of KAgent uses the conversation |
| | GET `/api/coders/:id/review` (S38) | | `CoderReview`; 404 for a session that is not a coder run |

```ts
type SkillView = { name: string; description: string; whenToUse?: string;
  source: "bundled" | "user" | "imported"; path: string; realPath: string; enabled: boolean;
  shadowed: boolean; sizeBytes: number; body?: string; uses: number; lastUsed?: string;
  symlink: boolean; updatedAt?: string; importedFrom?: { source: string; path: string; hash: string } };
type SourceView = { id: string; name: string; url: string; ref: string; subdir: string;
  commit?: string; lastSync?: string; lastError?: string; skills: [{ name: string; hash: string; syncedAt: string }] };
type MemoryView = { name: string; description: string; type: string; path: string; bytes: number;
  createdAt: string; updatedAt: string; source: "user" | "agent"; reviewed: boolean;
  validUntil?: string; supersededBy?: string; content?: string };
type TodoItem = { content: string; status: "pending" | "in_progress" | "completed"; priority: "high" | "medium" | "low" };
type TaskRun = { at: string; status: "running" | "ok" | "failed" | "cancelled" | "timeout" | "missed"; durationSec?: number;
  sessionId?: string; result?: string };
type AgentProfileView = { name: string; description: string; builtIn: boolean; source: "builtin" | "user";
  model: string; toolGroups: string[]; maxIterations: number; prompt: string };
type HookView = /* config.md section 11, Hook */;
type CoderToolView = { tool: "claude" | "codex"; installed: boolean; path: string | null; version: string | null };
type CoderProfileView = { name: string; description: string; builtIn: boolean; source: "builtin" | "user";
  tool: "claude" | "codex"; mode: "plan" | "edit" | "full"; model: string; effort: string; allowedTools: string[];
  args: string[]; env: Record<string, string>; prompt: string };
type CoderRunView = { id: string; parent: string; title: string; profile: string;
  status: "queued" | "running" | "done" | "failed" | "cancelled"; startedAt?: string; endedAt?: string; project: string };
type CoderReview = { tool: string; profile: string; project: string; branch: string | null; worktree: string | null;
  files: string[]; status: string; stat: string };
```

`SkillView`: `path` is the skill's folder as found and `realPath` the same with links
resolved; `updatedAt` is when `SKILL.md` was last written (RFC 3339). Empty optional
fields are left out. `GET /api/skills` lists the skills that own their name and, flagged
`shadowed`, those they hide, without bodies; every other skills route answers with the
body. A refused write is 400 (`invalid_input`) or 409 (`conflict`).

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
{"type":"calendarChanged","state":"off|connecting|connected|reconnect|error"}
```

`uiCommand` `show` args gain `section` (settings section id). `notice` with level
`tasks` is no longer used once S28 is done (`tasksChanged` replaces it).

Phase 5 adds no event: a coder run is reported with `subagentUpdated`, as a subagent is
(`coders.md` section 2).

## 8. Phase 6 endpoints and events (S41-S43)

Same transport, auth and error shape as section 3. S41 opens this section; S42 and S43 add
their rows. Spec of S41: `picker.md`.

### Endpoints

| Method path | Request | Response |
|---|---|---|
| PUT `/api/sessions/:id/model` | `{model?: string\|null, reasoningLevel?: string\|null}` | `SessionMeta` |
| POST `/api/sessions` | `{title?, model?, reasoningLevel?, project?}` (`project` from S42) | `SessionMeta` (201) |
| PUT `/api/sessions/:id/project` | `{path: string\|null}` (S42) | `SessionMeta` |
| PUT `/api/sessions/:id/pin` | `{pinned: boolean}` (S42) | `SessionMeta` |
| GET `/api/sessions?project=<path>` | (S42) | the top-level sessions of that folder, newest first, as the plain list |
| GET `/api/projects` | (S42) | `{roots: string[], projects: [{path, name, agentsMd: boolean}]}` |
| GET `/api/models` | | as `models.md` section 4; each model of every list also has `providerId` and `providerName` |
| GET `/api/sessions/:id/background` | (S43) | `{tasks: BackgroundTaskView[]}`: the background shell tasks of that session, running first in the order they started, then ended ones, the one that ended last first; at most 50 |
| GET `/api/background/:taskId/output?tail=4000` | (S43) | `{task: BackgroundTaskView, output: string}`: the last `tail` characters, through the redactor of the token guard |
| POST `/api/background/:taskId/stop` | (S43) | `BackgroundTaskView` (the task as it is after the stop, `killed`) |

The two writes are checked by one function, before anything is written:

- A key left out keeps its value; `null` or `""` clears it. A refused request changes
  nothing and makes no session.
- 404 `not_found` "session <id> not found" (PUT only).
- 409 `conflict` "the session of a subagent takes the model of its job: it cannot be
  changed" for a session with origin `subagent` (PUT only).
- 400 `invalid_input`: a body that is not an object, a key other than `model` and
  `reasoningLevel` (and `title` on POST), or a value that is neither a string nor null
  ("expected an object with model and reasoningLevel (strings, or null to clear)"); a
  model that cannot be used ("model '<ref>' cannot be used: <reason>", the reason being
  `unknown model`, `provider switched off`, `model switched off` or `not a chat model`);
  an effort the answering model does not offer ("model '<ref>' has no effort '<level>' (it
  offers: low, medium, high)" or "(it offers none)"). The answering model is the `model` of
  the body, else the session's, else `settings.chatModel`; with none of the three an effort
  is refused with "effort '<level>' needs a model: pick a model first".
- When `model` is given and `reasoningLevel` is not, a stored level the new model does not
  offer is cleared.
- Nothing changes and no event goes out when the meta would be the same.

The folder and the pin (S42, `sidebar.md` section 3):

- `PUT .../project`: `{path: null}` or `{path: ""}` detaches. A path that starts with `~/` is
  expanded and `canonicalize` gives what is stored. 400 `invalid_input`: no `path` key or a
  value that is neither a string nor null ("expected {path: a folder, or null to detach}"); a
  path neither absolute nor `~/...` ("path must be absolute, or start with ~/"); not a
  directory ("not a directory: <path as given>"); KAgent's own `<config>` or `<data>`, or
  inside them ("<path> is KAgent's own folder (<config> or <data>): a conversation cannot be
  attached to it"). 404 "session <id> not found". 409 `conflict` "the session of a subagent
  cannot be changed: it has the folder of the session that started it". The same path again
  changes nothing and sends no event; `updated_at` stays.
- `PUT .../pin`: 400 "expected {pinned: true or false}"; 404; 409 "the session of a subagent
  is not in the list: it cannot be pinned".
- `POST /api/sessions` with `project`: the checks of the PUT run first; a refused create makes
  no session.
- `GET /api/sessions?project=` with `parent` too: 400 "give parent or project, not both".
- `GET /api/projects`: the projects of `coders.md` section 3 (the roots of
  `settings.desktop.projectRoots`, empty: the built-in ones); `agentsMd` is true when
  `<path>/AGENTS.md` is a regular file.

### Events

```json
{"type":"sessionMetaChanged","sessionId":"...","meta":{/* SessionMeta */}}
```

The whole meta after the file was written, so a window applies it without reading anything
(S42 reuses it for the folder and the pin).

### S43: background tasks and notices

`BackgroundTaskView` is `{id, sessionId, command, status: "running"|"done"|"failed"|"killed", exitCode: number|null, startedAt, seconds, outputFile}`; `seconds` stops counting when the task ends.

- `tail` is absent (4000) or a whole number from 100 to 20000; anything else is 400
  `invalid_input` "tail must be a whole number from 100 to 20000".
- 404 `not_found`: "session <id> not found" for the list; "no background task '<taskId>'" for the
  other two (unknown, or ended long ago and forgotten: the registry keeps the latest 200).
- 409 `conflict` on stop: "the task already ended (<status>)".

```json
{"type":"backgroundTask","taskId":"bg_1a2b3c4d","sessionId":"...","status":"running|done|failed|killed"}
{"type":"notice","level":"info|warn|error","text":"...","key":"provider:gemini:quota","sessionId":"...","kind":"error|recovery|info"}
```

`backgroundTask` goes out at the start (`running`), on a stop (`killed`) and at the end (`done`,
`failed`), before the notification of the task reaches its session; a killed task still tells
no session. `notice` gained `key`, `sessionId` and `kind`, each left out when it has no value
(an old client ignores them; `notices.md`):

- `key` is what makes two notices the same one: `provider:<id>:<class>` with class `quota`,
  `key`, `busy`, `down` or `other` (the class of the error that sent the turn to `fallbackModel`),
  `hook:<name>`, `calendar:reconnect`, `calendar:report`. A notice with no key is never merged.
- `kind` is `error` (a `warn` or `error` level), `info`, or `recovery`: the loop sends `recovery`
  with the key of the error when the model that failed answers again ("<model> answers again"),
  once, and never without a previous error.
- `sessionId` is set when the notice belongs to a turn (a fallback, a refused choice of model).

## 9. Local models (S47)

Spec: `local-models.md`. Same transport, auth and error shape as section 3.

| Method path | Request | Response |
|---|---|---|
| GET `/api/local-models` | | `{ollama: {reachable, version, baseUrl}, gpu: {name, vramTotalMb, vramFreeMb, source} \| null, ramTotalMb, installed: [{id, sizeMb, jobs, listed, ref, usedFor}], recommended: [Recommended], pulls: [Pull]}` |
| POST `/api/local-models/pull` | `{id}` | 202 `{id, pulls}` once the download is queued (or, for a model Ollama already has but Settings -> Models does not list, once its row is added) |
| DELETE `/api/local-models/pull/:id` | | 204: stops the running pull (Ollama keeps the partial layers, a later pull resumes), takes a queued one out of the queue, or dismisses a failed row |
| DELETE `/api/local-models/:id` | | 204: removes an installed model of the list from Ollama and its row from the Ollama providers |

`Recommended` is the entry of `config/local-models.json` (`id`, `name`, `jobs`, `sizeMb`, `vramMb`, `context`, `tools`,
`languages`, `note`, `engine`) plus `downloadable` (false for speech to text), `fit` (`fits`, `tight`, `tooBig`, `slow`),
`fitNote` (null, or "<n> MB are in use by other programs at the moment" when the model needs more than is free now),
`recommendedFor` (the jobs it is the pick of: the largest that `fits`, preferring the owner's `persona.language`; for chat only
models with `tools`), `installed`, `installedId` (Ollama's name, e.g. `nomic-embed-text:latest`), `ref` (`provider::id`, null while
Settings -> Models has no row for it) and `usedFor` (the settings that name `ref`: `chatModel`, `fastModel`, `fallbackModel`,
`visionModel`, `liveModel`). `Pull` is `{id, state: "queued"|"pulling"|"failed", doneMb, totalMb, error?}`; a finished pull is
an installed model and a cancelled one is gone. The "Use for ..." buttons are `PUT /api/settings` with the existing keys.

Refusals: POST pull is 400 for an id outside the shipped list or a speech model ("KAgent downloads only the recommended list"),
409 when Ollama is not reachable, the model is installed, it is already queued or running, or the disk is short ("<n> MB are free
where Ollama keeps its models and <m> MB are needed"; the check is skipped when Ollama is not on this machine, and when the free
space cannot be read). One pull runs at a time; the others wait. DELETE pull is 404 when nothing is queued, running or failed
under that id. DELETE model is 400 outside the list, 404 when not installed, 409 while a setting names it ("choose another model
for that first") or Ollama is down, 502 when Ollama refuses. Ollama's own address is the Ollama provider's base URL (without
`/v1`), else `http://127.0.0.1:11434`; where the models live is `OLLAMA_MODELS`, else `~/.ollama/models`, else
`/usr/share/ollama/.ollama/models`.

When a pull ends well the daemon runs Fetch models on the Ollama provider (added from its template at that point when the owner
has none) and gives the new row `enabled: true`, the roles of its jobs (`chat` and `vision` are the `chat` role, `vision` with
`supportsImage`; an embedding has none) and, for a chat model, `supportsToolCall` and `contextWindow` from the list. A row that
already existed is only switched on. Nothing is made the Brain or anything else.

```json
{"type":"localModelsChanged","pulls":[{"id":"gemma3:4b","state":"pulling","doneMb":812,"totalMb":3300}]}
```

`localModelsChanged` carries the whole `pulls` list: at every change of state, and while a download runs at most twice a second.
A window reads `GET /api/local-models` again when a pull leaves the list (the installed models changed).
