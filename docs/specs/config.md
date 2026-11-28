# Spec: files, config and on-disk formats

## 1. Paths

| Name | Default | Override |
|---|---|---|
| config dir | `~/.config/kagent/` | `$KAGENT_HOME/config/` |
| data dir | `~/.local/share/kagent/` | `$KAGENT_HOME/data/` |
| token | `<config>/token` (0600, 32 random bytes hex) | |
| settings | `<config>/settings.json` (0600) | |
| providers | `<config>/providers.json` (0600) | |
| skills (user) | `<config>/skills/<name>/SKILL.md` | bundled: `<repo or /usr/share/kagent>/skills/` |
| sessions | `<data>/sessions/<id>.jsonl` + `<id>.meta.json`; images in `<id>.images/` (S46) | |
| audit | `<data>/audit.jsonl` | |
| changes | `<data>/changes/<id>.json` | |
| alerts | `<data>/alerts.json` (array, updated in place) | |
| router password | `<config>/router.secret` (0600) | |
| heartbeat | `<data>/heartbeat` (written every 60 s) | |
| sentinel | `<data>/sentinel/{honeytokens.json, baseline.json}` | |
| net | `<data>/net.sqlite` | |
| usage | `<data>/usage.sqlite` | the counters of the providers (section 15) |
| agent browser profile | `<data>/browser-profile/` | |
| screenshots | `<data>/screenshots/` | |
| logs | `<data>/logs/kagentd.log` (daily rotation, 7 files) | `RUST_LOG` |

All writes of JSON config are atomic (tmp + rename) and keep mode 0600.

## 2. settings.json

```json
{
  "schemaVersion": 1,
  "chatModel": "ollama::qwen2.5:7b-instruct",
  "fastModel": "",
  "liveModel": "",
  "liveVoice": "Kore",
  "permissionMode": "ask",
  "rules": [{"tool": "systemd", "prefix": "systemctl --user restart"}],
  "toolOverrides": {"shell": "ask"},
  "toolGroups": ["system","kernel","services","files","shell","hardware","skills","change","desktop"],
  "mcpToolGroups": ["system","kernel","services","files","hardware"],
  "privilege": "sudo",
  "maxIterations": 40,
  "approvalTimeoutSec": 600,
  "options": {"reasoningLevel": null, "maxOutputTokens": null, "temperature": null},
  "persona": {"name": "KAgent", "address": "", "language": "auto"},
  "startup": {"autostart": true, "greeting": {"enabled": true, "text": ""}},
  "voice": {"hotkey": "Meta+Space", "orbTheme": "orb", "pauseMedia": true},
  "desktop": {"editor": "", "projectRoots": []},
  "browser": {"port": 9333, "binary": ""},
  "watchers": {"enabled": true, "disabled": [], "quietHours": "23:00-07:00", "spoken": false},
  "sentinel": {"honeytokens": false, "persistenceWatch": true},
  "mikrotik": {"host": "", "user": "kagent-ro", "fingerprint": "", "flowPort": 2055, "containment": false}
}
```
- Unknown keys are preserved on save (forward compatible).
- `options.reasoningLevel` is no longer read (S41): the effort is chosen per conversation
  (`SessionMeta.reasoningLevel`, section 12). The field stays so that a value in an old file
  survives a load and a save, as `options.maxOutputTokens` does since S40.
- `permissionMode`: `read-only | ask | trusted`. `privilege`: `sudo | pkexec | none`.
- `toolOverrides`: tool -> `allow | ask | deny` (applied after mode, before rules;
  never relaxes PRIV/always_ask).
- `mcpToolGroups`: the tool groups `kagentd mcp` lists and `POST /api/tools/:name/call` runs
  (empty: the five above). The check applies to every call of that route, whatever the body
  says (v0.0.3). Whatever the group, a call of an outside agent that is not read-only asks in
  every permission mode and is satisfied by no stored rule (`tools.md` section 0).
- `desktop.editor`: `""` (auto) or `vscode|kate|kwrite|idea|pycharm|webstorm|phpstorm|clion|studio|zed`
  (spoken names like "VS Code" accepted); unknown names rejected with 400.
  `desktop.projectRoots`: folders whose children are projects; empty = built-in list.
- `browser.port`: 1024-65535 (else 9333), read at startup. `binary`: empty = auto.
- `startup.autostart`: keep the systemd user unit `kagentd.service` in sync - when
  true the daemon (re)writes the unit from its own executable path, runs
  `systemctl --user daemon-reload` and `enable`s it (starts at next login; it never
  starts a second copy while one runs); when false the unit is disabled. User-level
  only, no sudo. Applied at daemon start and when the setting changes via the API.
- `startup.greeting.enabled`: after the HTTP listener binds, a desktop notification
  greets the user. `text`: custom text; empty = auto ("Good morning/afternoon/
  evening, <persona.address>. KAgent is up and watching.").
- `persona.language`: `auto` (answer in the user's language), `sl`, `en`.
- Passwords/keys never live here (MikroTik password: Secret Service from S9; before S9
  `<config>/secrets.json` 0600).

## 3. providers.builtin.json (generated, read-only, shipped)

```json
{
  "schemaVersion": 1,
  "generatedFrom": "ZCode zcode-builtin.json revision <n>",
  "templates": [
    {"id": "anthropic", "name": "Anthropic",
     "api": {"type": "anthropic-messages", "baseUrl": "https://api.anthropic.com", "headers": {}},
     "keyUrl": "https://console.anthropic.com/settings/keys",
     "models": ["..."],
     "liveModels": [], "voices": []}
  ],
  "modelRules": [
    {"match": {"model": ".*glm-5.*"}, "config": {"contextWindow": 200000, "reasoningLevels": ["disabled","enabled"], "maxOutputTokens": 64000}},
    {"match": {"model": ".*", "apiType": "anthropic-messages"}, "config": {}},
    {"match": {"model": "glm-5\\.3", "apiType": "openai-chat-completions", "baseUrl": "https://opencode\\.ai/zen/go/v1/?"}, "config": {}},
    {"match": {"templateId": "opencode-go-chat", "model": "glm-5\\.3"}, "config": {"enabled": true}}
  ]
}
```
`api.type`: `anthropic-messages | openai-chat-completions | openai-responses | ollama-native`
(`ollama-native` = Ollama `/api/chat`, sends `options.num_ctx` = model contextWindow).
Rule precedence (lowest -> highest), same as ZCode: rules with only `model`; `model +
apiType`; `model + baseUrl (+apiType)`; `templateId + model` (exact model id, not regex);
then the personal model `config`. Within one precedence class, file order. Regex:
anchored, case-insensitive.

**ModelConfig fields** (all optional in rules, resolved with defaults):

| Field | Type | Default |
|---|---|---|
| `enabled` | bool | true |
| `contextWindow` | int | 128000 |
| `maxOutputTokens` | int | 8192 |
| `supportsToolCall` | bool | true |
| `supportsImage` | bool | false |
| `reasoningLevels` | string[] | [] (no reasoning option) |
| `reasoningPatches` | map level -> JSON object merged into the request body | per-API default (below) |
| `supportsLive` | bool | false |
| `liveProtocol` | `gemini-live` \| `openai-realtime` | none |
| `requestPatch` | JSON object always merged into the body | {} |
| `roles` | string[] subset of `chat, fast, live, vision, embedding` | `["chat"]`, live models `["live"]` |

Per-API reasoning defaults when no patch: openai-chat -> `{"reasoning_effort": level}`
except `disabled` -> nothing; anthropic -> `disabled`: `{"thinking":{"type":"disabled"}}`,
`enabled`: `{"thinking":{"type":"adaptive"}}`, other: `{"thinking":{"type":"adaptive"},"output_config":{"effort":level}}`;
openai-responses -> `{"reasoning":{"effort":level}}`.

**Import script mapping** (`scripts/import-zcode-providers.py`): ZCode `modelRules ->
{match:{model}}`, `modelApiRules -> {match:{model, apiType}}`, `providerSiteRules ->
{match:{model, baseUrl, apiType?}}`, `templateModelRules -> {match:{templateId, model}}`;
config flattening: `properties.contextWindow -> contextWindow`,
`properties.inputFormat.supportsImage -> supportsImage`, `properties.supportsToolCall`,
`optionSpecs.reasoningLevel.values -> reasoningLevels`, `optionSpecs.maxOutputTokens.max
-> maxOutputTokens`, `optionSpecs.*.map` dropped. If a ZCode `map` is a plain dict
literal containing `reasoningLevel` as a value, render one patch per level into
`reasoningPatches` (try `json.loads` after substitution; skip on failure). Skip
templates with access type `zhipu-account` and `builtinProviderModelRules`.

**Extras** (`EXTRA_TEMPLATES` and `EXTRA_RULES` of the script, written by hand):
gemini, ollama, lmstudio, groq, mistral, `custom` ("Custom (OpenAI-compatible)":
`openai-chat-completions`, no base URL, no models; `name` and `baseUrl` are required
when it is added, `apiType` may be given) and `nvidia` ("NVIDIA":
`https://integrate.api.nvidia.com/v1`, three models). Site rules of `nvidia`:
`poolside/laguna-xs-2\.1` has a 262144 context window and
`nvidia/nemotron-3\.5-super-vl-120b-a12b` reads images (the values ZCode holds for
them); every other NVIDIA model takes the catch-all rule.

## 4. providers.json (personal, editable)

`schemaVersion` 2 since S40: a file below 2 has the roles of its old fetch corrected
once at load (models.md section 3, item 4) and is saved as 2.

```json
{
  "schemaVersion": 2,
  "order": ["ollama", "anthropic"],
  "providers": {
    "ollama": {
      "name": "Ollama (local)", "templateId": "ollama", "enabled": true,
      "api": {"type": "openai-chat-completions", "baseUrl": "http://127.0.0.1:11434/v1", "headers": {}},
      "apiKey": "",
      "models": [{"id": "qwen2.5:7b-instruct", "name": "", "enabled": true, "config": {}}],
      "modelsFetchedAt": "2026-11-23T10:00:00Z", "lastError": ""
    }
  }
}
```
- Provider id: template id, or `<templateId>-2`... when added twice; `^[a-z0-9-]{1,40}$`.
- After S9 `apiKey` is `""` and the key is in Secret Service (attributes
  `app=kagent, kind=provider, id=<providerId>`); `"apiKeyRef": "secret-service"` marks it.
- Masking in API output: keys shorter than 12 chars -> `****`; else first 3 + `...` +
  last 4.

## 5. Sessions

`<id>.meta.json`: `{id, title, createdAt, updatedAt, messageCount}` (SessionMeta in
`session.rs`). `<id>.jsonl`: one `Message` per line (protocol.rs, camelCase,
`role: user|assistant|tool`, `toolCalls[]`, `toolCallId`, `toolName`, `isError`,
`via`, `providerState`, and since S46 `images`). Title = first user line, 60 chars.

`images` (S46, `vision.md`): the images of a user message, at most 4, left out when empty:
`{id, mime: "image/png"|"image/jpeg", width, height, bytes, description?, readBy?}`. The
pixels are files, `<data>/sessions/<sessionId>.images/<id>.png|jpg`, each with an
`<id>.json` that holds its `ImageRef`; the folder is deleted with the session. `description`
and `readBy` (the vision model's id) are set once when the Brain cannot read images itself
and the vision model described it (`vision.md` section 2); the file is never rewritten for
them, they are in the message line.

## 6. Audit log (`audit.jsonl`, S6)

```json
{"seq": 12, "ts": "2026-11-23T10:00:00.123Z", "kind": "toolCall", "sessionId": "...",
 "data": {"callId": "...", "tool": "sysctl", "input": {...redacted}, "status": "ok", "summary": "...", "durationMs": 42, "outputSha256": "..."},
 "prev": "<hash of seq 11>", "hash": "<sha256 hex>"}
```
- `kind`: `toolCall | approval | change | undo | alert | containment | start | stop`.
- A `toolCall` record's `data` carries `origin` (v0.0.3): `chat`, `voice` or `agent` (a call
  of an outside agent through MCP, session `mcp`). The `approval` record's `via` is as
  before; the card of an `agent` call has a summary that starts "Outside agent (MCP): ".
- An `approval` record's `data`: `{requestId, tool, summary, decision, via}`; `via` says
  how the card was answered (v0.0.2): `click` (the web UI or the API, also Cancel plan),
  `voice`, `telegram`, `rule` (a policy, such as a task's `onApproval: deny`), `timeout`
  (`approvalTimeoutSec` passed), `cancel` (the turn was stopped while the card waited).
- `hash = sha256(prev + "\n" + canonical_json(record without hash))`; canonical =
  keys sorted, no whitespace. First record `prev = "0"*64`, kind `start`.
- Redaction: input fields named `password|apiKey|token|secret|content` are replaced by
  `"<redacted:sha256-prefix>"`; full tool output is not stored (only its sha256).
- `verify` returns the first broken seq or OK + count.
- Writing (v0.0.2): the record is built, written as one line and `prev` advanced under
  one lock, so records written at once still chain; the `fsync` runs on a thread of its
  own (`audit-sync`, requests that queued up share one), never on the caller's thread.

## 7. Change journal (`changes/<id>.json`, S8)

```json
{"id": "chg_20261123_100000_ab12", "ts": "...", "sessionId": "...", "callId": "...",
 "tool": "sysctl", "summary": "sysctl vm.swappiness=10 (persist)",
 "op": {...priv request...}, "undo": {...priv request or user-level undo...},
 "privileged": true, "status": "applied|failed|undone|undo_failed",
 "before": {"measure": "..."}, "after": {"measure": "..."}, "undoneAt": null}
```
User-level (non-priv) undos use `{"kind": "user", "action": "restore_file", "path": "...", "backup": "<data>/changes/<id>.bak"}`
or `{"kind":"user","action":"command","argv":[...]}`.
A calendar change (S45, tool `calendar_change`) is recorded with `op` `{"kind":"user","action":"calendar","op":"add|update|delete","calendar","eventId"}` and `undo`
`{"kind":"user","action":"calendar","step":"delete","calendar","id"}` (of an add),
`{"..., "step":"patch","calendar","id","fields":{<old values, null where the field did not exist>}}` (of an update) or
`{"..., "step":"insert","calendar","event":{<the stored copy>}}` (of a delete). The key is `step`, not `op`: a record with an `undo.op` is a root-helper request. `undo_change` asks the handler registered for the `action` (`ToolServices.undo`); the module is that handler for `calendar` and refuses when the calendar is no longer switched on.

## 8. Alerts (`alerts.jsonl`, S13) - see `alerts.md` for the model.

## 9. Sentinel (S14)

`honeytokens.json`: `[{"path": "...", "inode": 123, "kind": "aws|ssh|kdbx|env", "createdAt": "...", "marker": "kagent-ht-<random>"}]`.
`baseline.json`: `{"takenAt": "...", "items": {"authorized_keys": ["sha256..."], "units": ["name@scope"], "timers": [...], "crontabs": {"user": "sha256"}, "suid": ["/usr/bin/..."], "ldPreload": "sha256|absent", "modules": [...], "sudoersD": {"file": "sha256"}, "pamD": {"file": "sha256"}}}`.
Re-baseline only by explicit owner action (`POST /api/sentinel/rebaseline`), never
automatically after an alert.

## 10. net.sqlite (S15)

```sql
CREATE TABLE devices(mac TEXT PRIMARY KEY, first_seen INTEGER, last_seen INTEGER, last_ip TEXT, hostname TEXT, vendor TEXT, note TEXT);
CREATE TABLE flows_5m(bucket INTEGER, src TEXT, dst TEXT, dport INTEGER, proto INTEGER, bytes INTEGER, packets INTEGER,
  PRIMARY KEY(bucket, src, dst, dport, proto));
CREATE TABLE dest_seen(src TEXT, dst TEXT, first_seen INTEGER, PRIMARY KEY(src, dst));
CREATE INDEX flows_src ON flows_5m(src, bucket);
```
Retention: delete `flows_5m` older than 14 days hourly. `bucket` = unix time / 300 * 300.

## 11. Phase 4 paths, settings and files (S17-S33)

### Paths added

| Name | Path |
|---|---|
| imported skills | `<config>/skills-imported/<name>/` (a directory, or a symlink to a skill elsewhere) |
| skill sources | `<config>/skill-sources.json` (0600): `{schemaVersion, sources: [...], imports: [{name, source, path, hash}]}`; clones under `<data>/skill-sources/<id>/`; copies are staged in `<config>/skills-imported.staging/` |
| skill usage | `<data>/skill-usage.json`: `{"<name>": {"uses": 3, "lastUsed": "<rfc3339>"}}` |
| memory | `<config>/memory/{MEMORY.md, user.md, machine.md, <topic>.md}` (`memory.md`) |
| subagent profiles | `<config>/agents/<name>.md` (`agents.md`) |
| plans | `<data>/plans/<sessionId>-<n>.md` |
| widgets | `<data>/widgets/<sessionId>/<n>.html` |
| background output | `<data>/task-out/<taskId>.log` |
| Telegram pairing | `<config>/telegram.json` (0600): `{botUsername, chatId, userId, pairedAt}`; token in Secret Service (`app=kagent, kind=telegram, id=bot`), file fallback `<config>/telegram.secret` (0600) when `KAGENT_HOME` is set |
| Brave Search key | Secret Service (`app=kagent, kind=search, id=brave`), same file fallback `<config>/search.secret` |
| Google Calendar connection (S45) | Secret Service (`app=kagent, kind=calendar, id=oauth`), file fallback `<config>/calendar.json` (0600) when `KAGENT_HOME` is set or no keyring answers: one JSON text `{"clientSecret", "refreshToken", "account"}`. The access token is never stored. `calendar.md` section 1 |
| Google Maps key (S45) | Secret Service (`app=kagent, kind=maps, id=places`), file fallback `<config>/maps.secret` (0600) |
| calendar state (S45) | `<data>/calendar-state.json`: where `calendar.home` was resolved to (`homeFor`, `homeLatLng`), the language last used (`lastLanguage`), the last morning report (`lastReport: {at, text, sent}`), the day it was delivered or given up (`reportDay`), the one-time "no report" notice (`toldSkip`) and the report that waits for a voice session (`spoken: {day, text}`) |

### Settings sections added

New keys are typed sections read with `Settings::section::<T>()` (`foundations.md`
section 3): defaults for missing fields, unknown keys preserved, typos rejected.

```json
{
  "skills": {"disabled": [], "indexBudgetChars": 3000},
  "memory": {"enabled": true, "recallCount": 3, "recallChars": 1500},
  "compaction": {"enabled": true, "thresholdPct": 80},
  "voice": {"brain": "live", "toolGroups": ["desktop", "tasks", "system"], "context": true, "vocabulary": true, "pauseMedia": true, "volume": 100},
  "browser": {"port": 9333, "binary": "", "backend": "own", "attachProfile": "", "attachTrust": "strict"},
  "tasks": {"onApproval": "deny", "deliver": ["desktop", "session"], "timeoutMin": 30, "maxConcurrent": 2},
  "telegram": {"enabled": false, "notifyAlerts": true, "forwardApprovals": false, "approvalForwardSec": 20, "allowPrivileged": false},
  "calendar": {"enabled": false, "clientId": "", "calendars": ["primary"], "defaultCalendar": "primary", "defaultDurationMin": 60, "home": "", "travelMarginMin": 10,
               "report": {"enabled": true, "weekdays": "07:45", "weekends": "09:00", "spoken": true, "telegram": true, "tomorrow": true, "tasks": true, "leaveBy": true}},
  "subagents": {"maxConcurrent": 4},
  "search": {"engine": "duckduckgo", "searxUrl": ""},
  "hooks": {"events": {}}
}
```

| Key | Values |
|---|---|
| `skills.disabled` | names (not paths) hidden from the prompt and from `load_skill` |
| `skills.indexBudgetChars` | 500..20000 |
| `memory.recallCount` | 0..8; `memory.recallChars` 0..8000 |
| `compaction.thresholdPct` | 50..95 |
| `voice.brain` | `live` (the default) \| `auto` \| `routed` (`voice.md`); `voice.toolGroups` = groups declared to the live model in `live` and `auto` (unknown names are refused with 400 and on load; `[]` is no tools); existing keys `voice.hotkey`, `voice.orbTheme` stay untouched |
| `voice.volume` | 0..100, default 100: the loudness of KAgent's own speech, the playback gain of the page (`voice-control.md` section 2); not the system volume (that is the `audio` tool). Above 100 is refused with 400 |
| `browser.backend` | `own` \| `attach`; `browser.attachProfile` = user data dir of the browser to attach to, empty = first found (`browser.md`); it must be an absolute path or start with `~/` (else 400, and refused on load); `browser.attachTrust` `strict` \| `normal` |
| `tasks.onApproval` | `deny` \| `wait`: default policy for unattended runs (`automations.md`) |
| `tasks.timeoutMin` | 1..1440; `tasks.maxConcurrent` 1..8; `tasks.deliver` only `desktop`, `session`, `telegram` (outside these: 400, and refused on load) |
| `telegram.*` | `bots.md`; `approvalForwardSec` 5..600 |
| `calendar.*` | `calendar.md` section 2. `defaultDurationMin` 5..1440; `travelMarginMin` 0..120; `report.weekdays` and `report.weekends` are `HH:MM` or `""`; `defaultCalendar` must be one of `calendars`; an unknown key is refused (`deny_unknown_fields`, also in `report`). `calendar.enabled` and `calendar.clientId` are protected keys of the `settings` tool; `calendars`, `defaultCalendar`, `defaultDurationMin`, `home`, `travelMarginMin` and `report.*` are free. A read-only default is refused when an event is added, not on save. The first successful connection sets `enabled` and adds the group `calendar` to `toolGroups` and `voice.toolGroups` |
| `subagents.maxConcurrent` | 1..8 |
| `search.engine` | `duckduckgo` \| `brave` \| `searxng` \| `none` (`web.md`); `search.searxUrl` is empty, or an http or https address without a password (else 400, and refused on load); the Brave key is set with `PUT /api/web/brave-key`, not here |
| `hooks.events` | `hooks.md` |

`PUT /api/settings` merges objects recursively; `toolGroups` with an unknown name and
`toolOverrides` with a value other than `allow`/`ask`/`deny` are rejected with 400
(`null` instead removes that tool's override, which a merge cannot do).
`toolGroups: []` means all groups.

### Hook (in `hooks.events.<Event>[]`)

```json
{"id": "hook_1a2b3c4d", "event": "PreToolUse", "matcher": ["shell", "write_file"],
 "command": "/home/me/bin/notify-hook", "args": ["--quiet"], "shell": "",
 "timeoutMs": 5000, "async": false, "enabled": true}
```
`id` is assigned by the daemon (`POST /api/hooks`) and required in the file; `event`
is required and is the key the record sits under; `enabled` absent = on; `args` cannot
be used with `shell`; `shell` is one program, run as `<shell> -c <command>`; `matcher`
absent = every tool; `command` is an executable path or name resolved through `PATH`
when `shell` is empty; `timeoutMs` 100..60000. Unknown keys are rejected.

### Session meta fields added (all optional, default empty)

```json
{"origin": "chat", "parent": null, "summary": null, "summaryUpto": null,
 "todo": [{"content": "Check disk", "status": "in_progress", "priority": "medium"}],
 "planning": false, "approvedPlan": null}
```
`origin`: `chat | voice | task | alert | telegram | subagent`. `summary` + `summaryUpto`
come from compaction (`compaction.md`), `approvedPlan` from plan mode (`plan-todo.md`):
`{"path": "<data>/plans/<sessionId>-<n>.md", "text": "<the plan>", "approvedAt": "<RFC 3339>"}`
once a plan was approved; `n` is the first number the session has not used. It is
cleared when the plan expires (`plan-todo.md` section 2).

### Coding agents (S35-S39; `coders.md`)

Settings section `coders`, typed like the others:

```json
"coders": {"defaultProfile": "claude-edit", "timeoutMin": 60, "launchesPerMinute": 6,
  "loginShell": false, "claudeBinary": "", "codexBinary": ""}
```

| Key | Values |
|---|---|
| `coders.defaultProfile` | a profile name (`^[a-z0-9][a-z0-9-]{0,63}$`), default `claude-edit`; a name that is no profile when `coder_run` runs is an error of the call, not of the settings |
| `coders.timeoutMin` | 1..1440 |
| `coders.launchesPerMinute` | 1..30 |
| `coders.loginShell` | boolean: the tool runs through a login shell (`coders.md` section 1) |
| `coders.claudeBinary`, `coders.codexBinary` | empty (the name on `PATH`) or an absolute path (else 400, and refused on load); `codexBinary` from S36 |
| `coders.notifyDesktop`, `coders.notifyTelegram` | boolean (S38; default true and false) |
| `coders.notifyAfterMin` | 0..120 (S38; default 2) |

Paths: a run record `<data>/coders/<childId>.json` for each running coder run, removed
when it ended (`coders.md` section 2); the profiles of the owner
`<config>/coders/<name>.md` (S36), a folder protected like `<config>/agents`.

Session meta: the session of a coder run carries `coder`; every other session has none:

```json
"coder": {"tool": "claude", "sessionId": "<the tool's own conversation id>",
  "profile": "claude-edit", "project": "/home/me/code/app", "resume": null}
```
S38 adds `branch` (string or null), `worktree` (the folder, or null) and `files` (the
files the tool edited).

## 12. Phase 6 session fields (S41-S43)

S41 opens this section; S42 and S43 add their fields. All are optional (`#[serde(default)]`),
so a meta file from before loads.

```json
{"id": "...", "title": "...", "createdAt": "...", "updatedAt": "...",
 "model": "anthropic::claude-opus-4-6", "reasoningLevel": "high",
 "planning": false, "approvedPlan": null}
```

| Field | Meaning |
|---|---|
| `model` | a model ref `<providerId>::<modelId>` the conversation chose, or `null`: the default chat model answers |
| `reasoningLevel` | one of the levels the answering model offers (`effective.reasoningLevels`), or `null`: nothing is sent and the provider's own default applies |
| `project` | (S42) the folder attached to the conversation: an absolute canonical path, or `null` |
| `pinned` | (S42) the session stands in the section "Pinned" of the sidebar; default `false` |
| `imagesNoticeShown` | (v0.0.3) the composer showed "Images are sent to <provider>." in this session; default `false` |

- The session of a subagent never has either: the route refuses it and the child is made
  with none. Voice, tasks, alerts and Telegram keep the default unless the session has a choice.
- A choice that cannot be used when a turn starts (the provider or model was switched off
  since) makes the default answer, and a `notice` says why.
- The last choice of the owner is in the browser, not in a file: localStorage `kagent.picker` =
  `{"model": "<ref> or null", "reasoningLevel": "<level> or null"}`; a new conversation
  starts with it.
- S42: a subagent has the `project` of its parent and is never pinned. The browser
  remembers the sidebar, not the daemon: localStorage `kagent.sidebar` =
  `{"view": "projects" | "recent", "collapsed": boolean, "closed": ["<path>", ...]}`.

## 13. config/local-models.json (S47, shipped, read-only)

The list of recommended local models (`local-models.md` section 2), compiled into the daemon with
`include_str!` and parsed with `deny_unknown_fields`, so a typo fails the first start and the unit test.

```json
{"schemaVersion": 1, "models": [
  {"id": "gemma3:4b", "name": "Gemma 3 4B", "jobs": ["vision", "chat"], "sizeMb": 3300, "vramMb": 4500,
   "context": 8192, "tools": false, "languages": ["en", "sl"], "note": "...", "engine": "ollama"}]}
```

`id` is unique and, for `engine: "ollama"`, the name Ollama pulls. `jobs` is a non-empty list of `chat`,
`vision`, `embedding`, `stt`. `vramMb` is what the model needs loaded with `context`. `tools` is whether it
calls tools reliably. `engine` is `ollama` (downloadable) or `whisper` (listed only, no Download). The sizes
and memory numbers are hand-kept estimates, checked against Ollama's library when the list is updated with a
release. No new key in `settings.json`: the page saves `chatModel`, `fallbackModel` and `visionModel`, which
exist.

## 14. settings.screenWatch (S48)

```json
"screenWatch": {"enabled": true, "cloudConfirm": false, "telegram": false, "defaultMinutes": 20, "everySec": 10}
```

Typed section (`screen_watch/settings.rs`, `deny_unknown_fields`, a typo is refused with the suggestion like the other sections).
`enabled`: off refuses `screen_watch start` (a running watch goes on). `cloudConfirm`: a hit is confirmed by the cloud vision
model of the `screen` tool with the one frame of the hit before it is reported; never a model on this machine or network.
`telegram`: a hit and the end of a watch also go to the paired chat. `defaultMinutes` 1..=120 and `everySec` 5..=60 are what a `start`
without them gets; out of range makes the file invalid. Protected keys of the `settings` tool: `screenWatch.cloudConfirm` and
`screenWatch.telegram` (they let a frame, or words about it, leave the machine); the other three are free. "Local" is decided by
`screen_watch::vision::is_local_url`: the provider's base URL is a loopback, private (10/8, 172.16/12, 192.168/16, fc00::/7) or
link-local address, or `localhost`; a host name that needs a lookup counts as not local.

`<data>/screen-watch.json` exists only while a watch runs: the watch as JSON (id, session, goal, target, window title, times, last
note). It holds no image and is deleted when the watch ends.

## 15. usage.sqlite and settings.usage (v0.0.3)

Spec: `usage.md`.

`<data>/usage.sqlite` (WAL), one table, created at the first start:

```sql
usage(win TEXT, start TEXT, provider TEXT, model TEXT, source TEXT, requests INTEGER, input_tokens INTEGER, output_tokens INTEGER,
      PRIMARY KEY(win, start, provider, model, source)) WITHOUT ROWID
```

`win` is `minute`, `day` or `month`; `start` is the window in the machine's local time (`YYYY-MM-DDTHH:MM`, `YYYY-MM-DD`,
`YYYY-MM`); `source` is `chat`, `voice`, `subagent`, `task` or `other`. Each answer that ended well adds one request and its
tokens to the three windows it falls in. Rows older than 13 months are deleted at start. When the file cannot be opened the
counters are kept in memory and a warning is logged.

`settings.usage` (a typed section, free for the `settings` tool): `knownLimits`, a map from a provider id to a list of
`{window, unit, total}` (`window`: `minute`, `day`, `month`; `unit`: `requests`, `tokens`; `total`: 1 to 1000000000000; one
limit for each window and unit of a provider). Unknown keys and values are refused (`deny_unknown_fields`). KAgent counts its own
requests and tokens of the current window against each and shows them with `source: "kagent"`. Never prefilled.

## 16. tickets/<id>.json (v0.0.3)

Spec: `tickets.md`. `<data>/tickets/<id>.json`, mode 0600, one file per project, written atomically (a temporary file in the same
folder, then rename). `<id>` is `inbox`, or `<folder name, [a-z0-9-], at most 24>-<8 hex of the sha-256 of the path>`.

```json
{"project": "/home/me/code/kagent", "prefix": "KA", "next": 13, "tickets": [
  {"key": "KA-12", "project": "/home/me/code/kagent", "title": "...", "description": "markdown", "state": "todo", "priority": "none",
   "labels": [], "parent": null, "links": [{"kind": "range", "value": "master..v0.0.3", "label": ""}], "assignee": null, "doneWhen": [],
   "notes": [{"at": "...", "by": "owner", "text": "..."}], "history": [{"at": "...", "by": "owner", "what": "created"}],
   "createdAt": "...", "updatedAt": "..."}]}
```

`next` only grows, so a key is never reused after a delete. The prefix is fixed when the file is made. `state` is `backlog`, `todo`,
`in_progress`, `in_review`, `done`, `dropped`; `priority` `none`, `low`, `medium`, `high`, `urgent`; a link `kind` `conversation`,
`commit`, `range`, `url`. A file that cannot be read is skipped with a warning and never overwritten. There are no settings keys.

The change journal (section 7) gains one field for a file edit: `op.afterBackup`, the path of `<data>/changes/aft_<uuid>`, the text
that `write_file` or `edit_file` wrote (the text before is the backup of the undo). The diff view reads both (`diff.md` section 3).
