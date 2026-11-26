# Spec: agent foundations for phase 4 (S17)

Fixes the gaps the phase 4 features would otherwise build on, found in the review of
2026-11-24: no way to await a turn, "allow for this session" being daemon-wide,
settings patches replacing whole objects, no shared notification path, no session
origin, a tool-budget trap for new groups, and the systemd unit being rewritten by
every daemon start.

## 1. Turn handle (`kagent-core/src/agent/turn.rs`, used by `agent/mod.rs`)

```rust
pub struct TurnResult { pub status: TurnStatus, pub error: Option<String>, pub text: String, pub usage: Usage }
pub struct TurnHandle { /* receiver + shared result */ }
impl TurnHandle { pub async fn wait(self) -> TurnResult; }
impl Agent { pub async fn wait_idle(&self, session_id: &str) -> TurnResult; }
pub enum SendOutcome { Started { turn_id: String, handle: TurnHandle }, Queued }
```
- `TurnStatus` is the type `TurnFinished` already carries (`Completed`, `Failed`,
  `Cancelled`).
- `text` = the TextDelta text of the turn, joined. It is filled even when the turn is
  cancelled or fails, so callers keep partial answers.
- `wait` resolves exactly once: on normal end, on a panic inside the turn, when the
  task is aborted at shutdown, and when `stop_all` cancels the turn.
- `wait_idle(session)` resolves when the session has no running turn and nothing
  queued, and returns the result of its last turn. It is what callers that only hold
  a session id use instead of polling `is_running`.
- Panic guard: `turn_loop` runs under `futures::FutureExt::catch_unwind` with
  `AssertUnwindSafe`. A panic gives `Failed` with error `internal error (see log)`,
  clears `running`, emits `TurnFinished` and logs `tracing::error!` with the payload.
  The session accepts a new turn afterwards.
- `tasks::execute` and `krunner::ask` switch from their `is_running` loops to
  `wait_idle`; their observable behaviour does not change in this stage.

## 2. Session-scoped allowances (`kagent-core/src/permission.rs`)

- `session_rules` becomes `HashMap<String /* session id */, Vec<AllowRule>>`. Step 8
  of `evaluate` ("a matching stored rule gives Allow") looks only at the calling
  session's list, so `evaluate` takes `session_id`.
- A call whose session id is empty or unknown never matches a session rule.
- api.md's `allowSession` note now reads: allowed for the rest of this daemon run, in
  this session only.

## 3. Settings sections (`kagentd/src/settings.rs`)

- `patch` merges objects recursively (`merge_patch(base, patch)`): a nested object
  updates only the keys it names; arrays and scalars are replaced. A patch of
  `{"startup":{"greeting":{"enabled":false}}}` keeps `autostart` and the greeting text.
- `Settings::section::<T>(key) -> Result<T, SectionError>` is a typed view of one
  settings object. `T: DeserializeOwned + Default`, its fields carry
  `#[serde(default)]`. Every section of config.md section 11 is read through it, and
  so are the existing `voice`, `browser` and `tasks` keys.
  - A key that is within edit distance 2 of a field of `T` is an error:
    `unknown key "voice.bran" (did you mean "brain"?)`.
  - Any other unknown key is kept and written back unchanged.
  - Load and `PUT /api/settings` both validate every typed section: 400 on a typo.
- `PUT /api/settings` also rejects, with 400: a `toolGroups` entry that is not a known
  group, and a `toolOverrides` value outside `allow`, `ask`, `deny`. `null` removes that
  tool's override (S19: the Tools section needs it; a merge cannot drop a key).
- `toolGroups: []` means all groups everywhere, including `GET /api/tools`, which then
  reports `enabled: true`.

## 4. Notification hub (new `kagentd/src/notify.rs`)

```rust
pub enum Urgency { Low, Normal, Critical }
pub enum Channel { Desktop, Telegram }
pub struct Notification { pub title: String, pub body: String, pub urgency: Urgency, pub channels: Vec<Channel> }
impl Notifier { pub fn new() -> Arc<Self>; pub async fn send(&self, n: Notification) -> anyhow::Result<()>; }
```
- `Notifier` lives in `AppState`. Channel `Desktop` runs `notify-send` (summary =
  title, body = body, `--urgency`), skips when `KAGENT_NO_NOTIFY` is set, and never
  fails the caller (errors are logged).
- `Channel::Telegram` is accepted from S17 on and does nothing until S29 registers the
  sink.
- `tasks::notify` is deleted; its callers use the hub. `AlertManager` keeps its own
  desktop path because of quiet hours.

## 5. Session origin (`kagent-core/src/session.rs`, `protocol.rs`)

- `Via` gains `Task`, `Telegram`, `Agent` (serde names `task`, `telegram`, `agent`).
- `SessionMeta` gains `origin` and `parent` (config.md section 11), both
  `#[serde(default)]`, so existing files load. `origin` default is `chat`.
- `SessionStore::create_with(title, origin, parent) -> SessionMeta`; `create` stays.
- Writers set the origin: a live voice session creates `voice`; a task run creates
  `task`; an alert investigation creates `alert`.
- `GET /api/sessions?parent=<id>` lists the children of a session. Without the
  parameter, sessions with origin `subagent` are not listed.

## 6. Tool budget for new groups (`kagent-core/src/context.rs`)

- `budget_tools` appends groups missing from `GROUP_PRIORITY` after the known ones, in
  registry order, instead of never keeping them.
- Rule for every later stage that adds a tool group: add it to `group_of`
  (`tools/mod.rs`), to `DEFAULT_TOOL_GROUPS` (`settings.rs`) and to `GROUP_PRIORITY`,
  and extend `every_builtin_tool_has_a_group` to cover the tools the daemon registers.

## 7. Autostart guard (`kagentd/src/startup.rs`, `main.rs`)

- At daemon start the systemd unit is rewritten only when the setting is on and
  - `KAGENT_HOME` is unset, and
  - the running binary is not inside a `target` directory, or the existing unit already
    points into that same `target` directory.
- A unit that points to a release binary is never replaced by a `target/` binary.
- `PUT /api/settings {startup.autostart}` is an explicit owner action and still writes,
  except when `KAGENT_HOME` is set.
- Rationale: a debug or smoke-test daemon start currently repoints the owner's
  autostart service at `target/debug/kagentd`.

## 8. Tests (unit, no root, no network)

- `turn.rs`: `wait` resolves with each status; a cancelled turn keeps its partial text;
  a tool that panics gives `Failed` and the session then runs a new turn; `wait_idle`
  returns after a queued second turn.
- `permission.rs`: an allowSession rule granted in session A does not allow the same
  call in session B; it still allows it in A.
- `settings.rs`: a nested patch keeps its siblings; a typo is rejected with the
  suggestion; a far unknown key is preserved through save and load; invalid
  `toolGroups` and `toolOverrides` are rejected; `null` removes one override.
- `context.rs`: a group missing from `GROUP_PRIORITY` is kept when the budget allows.
- `startup.rs`: the guard table (home set, target binary, release unit, same target).
- `notify.rs`: the desktop sink builds the expected argv; `KAGENT_NO_NOTIFY` skips.
- `session.rs`: meta without `origin`/`parent` loads with defaults; the parent filter
  and the hidden `subagent` origin.
