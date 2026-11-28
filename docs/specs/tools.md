# Spec: tools

Every tool the model can call. Implements `kagent_core::tool::Tool`. This file is the
source of truth for names, input schemas, per-call assessment and output format.

## 0. Common rules

- **Names** are `snake_case`, stable. Never rename once shipped (rules and audit refer
  to them).
- **Input schema**: JSON Schema subset handled by `util::validate_schema` (`type`,
  `properties`, `required`, `enum`, `items`). Every schema is `{"type":"object", ...}`.
  Every property has a `description` (the model reads it). No `additionalProperties`.
- **`timeout_ms`** may be added by the model to any tool; the agent clamps it to the
  contract's `max_timeout_ms`. Do not declare it in schemas.
- **Output**: plain text for the model, compact, sections as `## name`, tables as
  aligned columns, no ANSI colors. JSON only when the data is naturally nested. Max
  bytes = contract `max_output_bytes` (default 24 000); the agent truncates head+tail.
- **Errors the model can fix** (bad input, missing unit) -> `ToolOutput::error("...")`
  with a hint. **Infrastructure errors** (spawn failed) -> `Err(anyhow)`; the agent
  formats `error: ...`.
- **Result kinds** (v0.0.3, `agent-loop.md` B): besides `ok` and `error` a tool answers with
  `ToolOutput::not_done(what, next)` (`not done: <what>. Next: <next>.`, nothing changed,
  status `notDone`), `ToolOutput::shown(what)` (`shown to the owner: <what>. Nothing was
  changed.`, status `shown`: a page, a window or a notification only displayed) and
  `ToolOutput::partial(done, missing, next)` (`partly done: <done>. Missing: <missing>.
  Next: <next>.`, status `partial`). Rules for every tool: an error never reads as an
  empty result ("no matches" only when the command succeeded), a result never says
  "done" before the effect exists (queued, asked, shown: say so), a refusal says what to
  do instead, and a list or text that is cut says how much was cut and how to get the
  rest (`util::more_note`, `util::cut_with_note`).
- **Missing program** (e.g. `nvidia-smi` absent): return `ok` with `unavailable:
  <program> not installed` for that section, never fail the whole tool.
- **Per-call assessment** (`assess`): fields `read_only, needs_approval, always_ask,
  privileged, concurrency_safe, risk, summary, rule_key`. Columns below:
  RO = `CallAssessment::as_read_only()`; MUT = mutating (needs approval, not
  concurrency safe); PRIV = privileged (always asks, never satisfied by a rule).
- **Origin of a call** (`permission::Origin`, v0.0.3): `chat`, `voice` (`voice-control.md`
  section 4) or `agent`, a call of an outside coding agent through `kagentd mcp`
  (`POST /api/tools/:name/call`, session `mcp`). For `agent` the engine decides in this
  order: a `deny` override refuses; a read-only call runs (an `ask` override asks); read-only
  mode refuses; plan mode and `destructive` as for any call; then every other call ASKS, in
  every mode including `trusted` and `full`. No stored rule, session allowance or `allow`
  override satisfies it. Its card has `canRemember: false`, its summary starts
  "Outside agent (MCP): ", and its audit record has `origin: "agent"` (`config.md` section 6).
- **Privileged execution** goes only through the root helper (`priv-ops.md`), never
  through `exec(...).privileged(true)` with arbitrary argv. Exception: `shell` with
  `sudo=true` (PRIV, always asks, shows the exact command).
- **Not-yet-implemented actions** (stage stubs): return `ToolOutput::error("<action> is
  not available yet")`; assessment must still be the final one (MUT/PRIV).
- **Groups** (`settings.toolGroups`, see `config.md`): only tools in enabled groups are
  sent to the model. Default enabled: `system, kernel, services, files, shell, hardware,
  skills, change, desktop`.

Summary / rule_key conventions: `summary` is what the approval card shows as the title
(e.g. `systemctl restart bluetooth.service`). `rule_key` is the stable prefix an
"always allow" rule matches (e.g. `systemctl restart bluetooth.service`). Rules are only
offered when the call is MUT and not PRIV.

## 1. Group `system` (S0, drafted in `tools/sys.rs`)

### system_info (RO)
Input: `{}`. Output JSON (pretty): `hostname, distro, kernel, arch, user, uptimeHours,
loadAvg[3], cpu{model, logicalCores}, memoryMiB{total, available, swapTotal, swapFree},
temperatures[{zone, celsius}], battery{capacityPercent, status} | null`.
Also used by `GET /api/system` and the system prompt.

### process_list (RO)
```json
{"type":"object","properties":{
 "sort":{"type":"string","enum":["cpu","mem","pid"],"description":"default cpu"},
 "filter":{"type":"string","description":"case-insensitive substring of name or command line"},
 "limit":{"type":"integer","description":"default 25, max 200"}}}
```
CPU% sampled over 500 ms from `/proc/<pid>/stat` (utime+stime, USER_HZ=100). Output:
first line `N processes matched; showing M`, then `PID USER CPU% RSS_MiB S COMMAND`.

### process_signal (MUT, destructive, risk high; PRIV when `sudo=true`)
```json
{"type":"object","required":["pid"],"properties":{
 "pid":{"type":"integer"},
 "signal":{"type":"string","enum":["TERM","KILL","HUP","INT","STOP","CONT","USR1"],"description":"default TERM"},
 "sudo":{"type":"boolean","description":"only for processes of another user"}}}
```
Refuse pid <= 1. summary `kill -SIG PID (comm)`, rule_key `kill -SIG`. With `sudo`,
execute via priv op `kill`.

### disk_info (RO)
`{"section": enum [devices, usage]}`. devices: `lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINTS,MODEL`;
usage: `df -hT -x tmpfs -x devtmpfs -x squashfs -x overlay -x efivarfs`.

### network_info (RO)
`{"section": enum [addresses, routes, listening, dns]}`. addresses `ip -br addr`, routes
`ip route`, listening `ss -tulpnH`, dns `resolvectl status` (fallback
`cat /etc/resolv.conf`).

## 2. Group `kernel` (S1; mutating actions S8)

### kernel_info (RO)
```json
{"type":"object","properties":{
 "section":{"type":"string","enum":["version","cmdline","taint","modules","log"],"description":"omit for version+cmdline+taint"},
 "filter":{"type":"string","description":"modules: name substring; log: case-insensitive substring"},
 "lines":{"type":"integer","description":"log lines, default 100, max 500"}}}
```
- version: `/proc/version`. cmdline: `/proc/cmdline`.
- taint: value of `/proc/sys/kernel/tainted` + decoded flags. Table (bit: letter
  meaning): 0 P proprietary module, 1 F forced load, 2 S out-of-spec system, 3 R forced
  unload, 4 M machine check, 5 B bad page, 6 U user request, 7 D kernel died (oops),
  8 A ACPI table overridden, 9 W warning, 10 C staging driver, 11 I firmware workaround,
  12 O out-of-tree module, 13 E unsigned module, 14 L soft lockup, 15 K live patched,
  16 X auxiliary, 17 T struct randomization, 18 N in-kernel test.
- modules: `/proc/modules` -> `name size refcount used_by state`, filtered, sorted by name.
- log: `journalctl -k -n <lines> --no-pager -o short-iso` (+ `--grep` when filter).

### kernel_module
```json
{"type":"object","required":["action","module"],"properties":{
 "action":{"type":"string","enum":["info","load","unload","set_params"]},
 "module":{"type":"string","description":"module name, e.g. btusb"},
 "params":{"type":"object","description":"load/set_params: key -> value strings"},
 "persist":{"type":"boolean","description":"set_params: write /etc/modprobe.d/kagent-<module>.conf"}}}
```
- info: RO, `modinfo <module>` (+ loaded? from `/proc/modules`, + current params from
  `/sys/module/<m>/parameters/*`).
- load: PRIV, risk high, priv op `modprobe`. unload: PRIV, destructive, priv op
  `modprobe` `remove:true`. set_params: PRIV, priv op `modprobe_conf` (+ reload hint).
- Validation: module `^[A-Za-z0-9_-]{1,64}$`; param key `^[A-Za-z0-9_]{1,64}$`, value
  `^[A-Za-z0-9_.,:/=-]{0,128}$`.
- summary e.g. `modprobe -r btusb`; never rememberable (PRIV).

### sysctl
```json
{"type":"object","required":["action"],"properties":{
 "action":{"type":"string","enum":["get","list","set"]},
 "key":{"type":"string","description":"e.g. vm.swappiness; list: prefix such as net.ipv4"},
 "value":{"type":"string","description":"set: new value"},
 "persist":{"type":"boolean","description":"set: also write /etc/sysctl.d/99-kagent.conf (default true)"}}}
```
- get: read `/proc/sys/<key with . -> />`. list: walk prefix dir, max 300 keys,
  `key = value` lines, skip unreadable.
- set: PRIV, risk high, priv op `sysctl_set`. Change journal records old value.
- key `^[a-z0-9_]+(\.[a-z0-9_-]+)+$`, value `^[A-Za-z0-9 _.,:/-]{1,256}$`.

## 3. Group `services` (S1; control actions S8)

### systemd
```json
{"type":"object","required":["action"],"properties":{
 "action":{"type":"string","enum":["status","list","start","stop","restart","reload","enable","disable"]},
 "unit":{"type":"string","description":"unit name; .service is assumed when no suffix"},
 "state":{"type":"string","enum":["running","failed","enabled","all"],"description":"list filter, default running"},
 "scope":{"type":"string","enum":["system","user"],"description":"default system"}}}
```
- status RO: `systemctl [--user] status <unit> --no-pager -n 20 -l`.
- list RO: failed -> `systemctl [--user] --failed --no-pager --plain`; running ->
  `list-units --type=service --state=running`; enabled -> `list-unit-files
  --state=enabled`; all -> `list-units --all --type=service`.
- start/stop/restart/reload/enable/disable: MUT. scope user -> `systemctl --user ...`
  directly, risk medium. scope system -> PRIV, priv op `systemctl`. stop/disable are
  destructive. Change journal records `is-active`/`is-enabled` before.
- unit `^[A-Za-z0-9@_.:-]{1,128}$`, refuse `systemctl` on units matching
  `^(kagentd|dbus|systemd-.*|polkit|sshd?)\.service$` for stop/disable (error with
  explanation).
- rule_key `systemctl <action> <unit>`.

### journal (RO)
```json
{"type":"object","properties":{
 "unit":{"type":"string"},
 "priority":{"type":"string","enum":["emerg","alert","crit","err","warning","notice","info","debug"],"description":"max priority, e.g. err shows emerg..err"},
 "since":{"type":"string","description":"journalctl --since value, e.g. '1 hour ago', 'today', '2026-11-20 10:00'"},
 "grep":{"type":"string","description":"regex filter on message"},
 "lines":{"type":"integer","description":"default 100, max 500"},
 "kernel":{"type":"boolean"},
 "boot":{"type":"integer","description":"0 = current boot, -1 = previous"},
 "scope":{"type":"string","enum":["system","user"]}}}
```
argv builder is a pure fn (tested): `journalctl --no-pager -o short-iso -n N [--user]
[-u U] [-p P] [--since S] [-g G] [-k] [-b B]`. `since` rejects `;|&$` and backticks.

### boot_analysis (RO)
`{"section": enum [time, blame, chain]}` default all: `systemd-analyze time`,
`systemd-analyze blame --no-pager | head -20`, `systemd-analyze critical-chain --no-pager`.

## 4. Group `files` (S1; edits S8)

### read_file (RO)
Refuses KAgent's credential files (`<config>/token`, the `*.secret` files) with an error, in
every mode: the agent never reads its own API token (`token-guard.md`). `search_files` leaves
out rows that come from them.
```json
{"type":"object","required":["path"],"properties":{
 "path":{"type":"string","description":"absolute or ~/..."},
 "start_line":{"type":"integer","description":"1-based, default 1"},
 "max_lines":{"type":"integer","description":"default 200, max 400"}}}
```
Output: `path (N lines total)` then `lineno<TAB>text`. Binary (NUL in first 8 KB) ->
`binary file, N bytes, type: <file -b>`. Permission denied -> error suggesting
`read_root` only for paths in the priv read allowlist (`priv-ops.md`).

### list_dir (RO)
`{path (required), hidden: boolean, limit: integer (default 200, max 500)}`. Output rows
`type size mtime name` (`d`/`f`/`l`), dirs first.

### search_files (RO)
`{pattern (required, regex), path (default ~), glob, max_results (default 100, max 200)}`.
`rg -n --no-heading --max-count 5 [-g glob] pattern path`, fallback `grep -rnE`.
Exit 1 is "no matches for /<pattern>/ in <path>" (an error). Exit 2 (or a time-out) is an
error of the search (unreadable folders, a bad pattern), never "no matches": without
matches the result is an error that says the search failed, with the first three lines
of stderr; with matches they are returned, followed by a line that the search did not
finish cleanly and why (v0.0.2).

### write_file (MUT, risk medium; S8)
`{path (required), content (required), mode: enum [overwrite, append, create]}`.
Only paths under `$HOME` (after canonicalize). Previous content saved in the change
journal. System paths -> error "use edit_file with a root path from the allowlist".
rule_key `write <canonical path>`.

### edit_file (MUT; PRIV for root paths; S8)
`{path (required), old (required, exact string, must occur once), new (required)}`.
Home paths: direct, backup in journal. Root paths only if in the priv `write_file`
allowlist: read via priv, replace, write via priv. Error when `old` occurs 0 or >1 times
(give counts).

## 5. Group `shell` (S1)

### shell
```json
{"type":"object","required":["command"],"properties":{
 "command":{"type":"string","description":"bash command line"},
 "cwd":{"type":"string"},
 "sudo":{"type":"boolean","description":"run as root (always asks)"}}}
```
- Runs `bash -c <command>`; with sudo: `sudo -n bash -c <command>` is NOT allowed (the
  sudoers drop-in only allows kagent-priv), so `sudo=true` maps to priv op `shell_root`
  which the helper only accepts when settings `allowRootShell=true` (default false;
  then error "root shell disabled, use a typed tool").
- assess: RO when every pipeline segment (split on `|`, trimmed) starts with an
  allowlisted program and the command contains none of `>`, `<`, `;`, `&`, backtick,
  `$(`, `\n`. Allowlist: `ls cat head tail grep egrep rg wc stat file df du free uptime
  uname ps pgrep lsblk lscpu lspci lsusb lsmod ip ss hostnamectl timedatectl localectl
  dmesg sensors nvidia-smi which id whoami date env printenv sort uniq cut awk sed(no -i)
  find(no -delete, -exec, -ok, -fprint) journalctl systemctl(only status|show|list-*|is-*|cat)
  apt(only list|show|policy) apt-cache dpkg(only -l|-L|-S|-s) snap(only list|info)
  flatpak(only list|info) docker(only ps|images|inspect|logs) podman(same) wpctl(only status|inspect)
  nmcli(only dev status|dev wifi list|con show|general) bluetoothctl(only devices|info|show)`.
- Otherwise MUT, risk medium; rule_key = first two words of the command.
- Asks in every mode, never rememberable (the destructive gate, `token-guard.md`): a
  command that names one of KAgent's credential files (`<config>/token`, the `*.secret`
  files: as an argument, inside `$(...)` or backticks, after a `cd`, through a wildcard),
  that runs `journalctl` on the daemon, or that contains `7878`, the port of KAgent's own
  API. With the token, a command could change the settings and hooks or answer an approval
  card.
- Tests: >= 10 RO and >= 10 MUT cases (`ls -la | grep x` RO; `ls > f` MUT; `sed -i`
  MUT; `systemctl restart x` MUT; `find / -delete` MUT; `echo $(id)` MUT).

## 6. Group `hardware` (S7)

### hardware (RO)
`{"section": enum [pci, usb, sensors, gpu]}`:
pci `lspci -nnk` condensed to `slot class: device [ids] driver=<in use>`; usb `lsusb`;
sensors `sensors -j` -> `chip/label: temp C (high X, crit Y)`; gpu
`nvidia-smi --query-gpu=name,driver_version,temperature.gpu,utilization.gpu,memory.used,memory.total,power.draw,clocks_throttle_reasons.active --format=csv,noheader`
(fallback: `/sys/class/drm/card*/device/{vendor,device}` + driver symlink).

### power (RO)
`{}`: batteries from `/sys/class/power_supply/*` (capacity, status, energy_now/full,
power_now -> watts, cycle_count), AC online, `powerprofilesctl get`, governor per
`/sys/devices/system/cpu/cpufreq/policy*/scaling_governor` (deduped), `upower -d`
summary if present.

### power_profile (get RO / set MUT, unprivileged; S8)
`{action: enum [get, set], profile: enum [power-saver, balanced, performance]}` ->
`powerprofilesctl get|set`.

### audio (RO)
`{}`: parse `wpctl status` into Sinks / Sources / Streams with ids, names, volume,
default marker `*`. Plus "apps using the microphone": `pw-dump` nodes with
`media.class == "Stream/Input/Audio"` -> `application.name`, `application.process.id`.

### bluetooth (RO)
`{}`: `bluetoothctl show` (powered, discoverable), `bluetoothctl devices` and for
connected ones (`devices Connected`) `info <mac>` condensed (name, battery if present).

### crashes (RO)
`{"since": string default "-7d", "pid": integer}`: list via
`coredumpctl list --json=short --since=<since> --no-pager` -> `time pid exe signal`;
with pid: `coredumpctl info <pid> --no-pager | head -60`.

### drivers (RO)
`{}`: `ubuntu-drivers list` (if present), `dkms status`, `mokutil --sb-state`,
loaded NVIDIA module version `/proc/driver/nvidia/version`, running kernel.

### disk_health (RO for the model, PRIV execution, always asks once per session)
`{"device": string (e.g. /dev/nvme0n1; omit = list)}`: omit -> list devices from
`lsblk -dn -o NAME,TYPE` (disks only). With device -> priv op `smartctl` -> summary:
model, health, temperature, power-on hours, percentage used / reallocated sectors,
media errors. assess: `read_only=false, privileged=true, always_ask=true, risk low`.

## 7. Group `skills` (S7)

### load_skill (RO)
`{"name": string (required)}` -> SKILL.md body (without frontmatter). Unknown name ->
error listing available skills.

## 8. Group `change` (S8)

### package
`{action: enum [search, info, install, remove], name: string, packages: string[]}`.
- search RO `apt-cache search --names-only <name>` (max 40 lines); info RO
  `apt-cache policy <name>` + `apt-cache show <name> | head -30`.
- install/remove PRIV, risk high (remove destructive): the tool first runs priv op
  `apt` with `simulate:true`; the simulation output (packages to install/remove/upgrade)
  becomes the approval card summary detail; after approval runs `simulate:false`.
  Package names `^[a-z0-9][a-z0-9+.-]{0,99}$`, max 20.

### measure (RO)
`{"kind": enum [boot, memory, thermal, cpu, network, all]}` -> one-line metrics used
for before/after: boot = `systemd-analyze time` total; memory = used/avail/swap;
thermal = max CPU and GPU temp; cpu = load + top 5 by CPU; network = `ping -c3 -W1
1.1.1.1` avg. The agent's system prompt tells the model to call `measure` before and
after a change.

### undo_change (MUT, PRIV if the original change was PRIV)
`{"change_id": string}` -> runs the journal's undo op. Summary `undo: <original summary>`.

### list_changes (RO)
`{"limit": integer default 20}` -> `id time status summary`.

## 9. Group `desktop` (S9) - all MUT unless noted, unprivileged, risk low

| Tool | Input | Implementation |
|---|---|---|
| `audio_control` | `{action: enum [set_default, set_volume, mute, unmute], target: string (id or name substring), volume: number 0-1.5}` | `wpctl set-default/set-volume/set-mute`; name -> id via `wpctl status` parse |
| `bluetooth_control` | `{action: enum [power_on, power_off, connect, disconnect], device: string (mac or name)}` | `bluetoothctl ...`; name -> mac via `devices` |
| `wifi` | `{action: enum [list RO, connect, disconnect, vpn_up, vpn_down], ssid, password, connection}` | `nmcli -t dev wifi list`, `nmcli dev wifi connect <ssid> [password <p>]`, `nmcli con up/down <name>`; password never logged (redact in audit) |
| `display` | `{action: enum [brightness, dark_mode, light_mode], value: integer 1-100}` | `brightnessctl set N%` or KDE D-Bus; `plasma-apply-colorscheme BreezeDark/BreezeLight` |
| `windows` | `{action: enum [list RO, activate, close, move_to_desktop], window: string, desktop: integer}` | KWin script via D-Bus (`org.kde.KWin /Scripting loadScript` + `run`); list returns `id caption app desktop` |
| `notify` | `{title, body, urgency: enum [low, normal, critical]}` | `org.freedesktop.Notifications.Notify` (zbus). RO-equivalent: no approval |
| `open` | `{target: string (URL, file path or .desktop app id)}` | `xdg-open` / `gtk-launch`; approval in ask mode |

## 10. Group `security` (S13)

### security_report (RO; PRIV parts optional)
`{"section": enum [ports, firewall, users, logins, boot, devices]}`:
ports `ss -tulpnH` with process; firewall `ufw status verbose` via priv `read_root`
alternative op `ufw_status`; users: accounts with login shells from `/etc/passwd`,
`getent group sudo`; logins: `last -n 20`, `journalctl -u ssh -g "Failed|Accepted" -n 50`;
boot: `mokutil --sb-state`, `/sys/kernel/security/lockdown`; devices: apps using mic
(pw-dump), camera (`fuser -v /dev/video*`), established connections with process
(`ss -tpnH state established`).

### list_alerts (RO)
`{"status": enum [open, all], "limit": integer}` -> alerts from `alerts.md` store.

## 11. Group `router` (S15)

| Tool | Assess | Input | REST call |
|---|---|---|---|
| `router_status` | RO | `{}` | `/system/resource`, `/interface` (name, running, rx/tx bytes), `/ip/firewall/connection` count |
| `lan_devices` | RO | `{}` | `/ip/dhcp-server/lease` + `/ip/arp`, merged by MAC, with first-seen from KAgent DB |
| `router_log` | RO | `{grep, limit}` | `/log` |
| `router_lists` | RO | `{list}` | `/ip/firewall/address-list` (optional `?list=`) |
| `traffic_top` | RO | `{device, minutes default 60}` | KAgent flow DB: top destinations/ports/bytes |
| `router_block_ip` | MUT, risk high, never rememberable | `{address, timeout default "1d", comment}` | PUT `/ip/firewall/address-list` `{list:"kagent-block", address, timeout, comment}`; undo = DELETE by `.id` |
| `router_unblock_ip` | MUT | `{address}` | find + DELETE |

`kagent-block` must be referenced by a drop rule the owner adds during setup (printed).

## 12. Group `trace` (S16)

### trace (RO for the model, PRIV execution, always asks)
`{"probe": enum [io-by-process, runq-latency, syscalls-top, tcp-retransmits, exec-snoop, open-snoop], "seconds": integer 1-10 (default 5), "path_prefix": string (open-snoop only)}`
-> priv op `bpftrace` with the script id; output summarized (top 15 rows / histogram).

## 13. Added after the spec (2026-11-24)

| Tool | Group | Assess | What |
|---|---|---|---|
| `what_changed` | system | RO | timeline: dpkg installs/upgrades/removals, boots, /etc files modified, KAgent changes, failed units; `{hours}` default 48 |
| `doctor` | system | RO | health score 0-100: PSI cpu/memory/io, disks, failed units, boot errors, temperature, swap, reboot-required, security updates, NVIDIA module |
| `list_alerts` | security | RO | registered by the daemon (alerts live there) |
| `router_*`, `lan_devices`, `traffic_top` | router | per section 11 | registered by the daemon; error "not configured" until Settings -> Network |

Tool budget: when the model's context window is small, the loop advertises whole groups
until the specs reach 35% of the window. Groups the latest user message mentions
(`context::hinted_groups`, English + Slovenian keywords: "play", "predvajaj", "browser",
"router", "install", ...) go first, then `context::GROUP_PRIORITY` order.

Platform-dependent tools (`package`, `display`, `windows`, `clipboard`, `input`,
`focus_context`, `what_changed`, `doctor`, `security_report`) pick their backend per
`docs/specs/platform.md`.

## 14. Apps, media, code and browser (group `desktop`, 2026-11-24)

| Tool | Assess | Input | What |
|---|---|---|---|
| `open` | MUT low; no approval for http(s) URLs, existing folders, non-executable files and app launches; approval for other URL schemes, executables, `.desktop` files | `{target?, app?}` | default app (`xdg-open`), or app by spoken name/.desktop id via `gio launch <desktop file> [target]` (resolves "VS Code", "vlc", "IntelliJ", snap/flatpak entries) |
| `media` | `status` RO; others MUT low, no approval | `{action: status\|play\|pause\|toggle\|stop\|next\|previous\|seek\|volume\|open, player?, seconds?, volume? (0-100), target?}` | MPRIS over `busctl --user --json=short`; default player = the playing one, then paused, then first; output: player state after the action |
| `projects` | RO | `{project?, search?}` | project folders (children of `settings.desktop.projectRoots`, default ~/SystemInterface, ~/projects, ~/src, ...) that have .git/Cargo.toml/package.json/...; fuzzy name lookup; `search` = regex over the project's files (code before docs, skips node_modules/target/.git...) -> `file:line: text` |
| `open_in_editor` | MUT low, no approval (opens a window) | `{project?, path?, line?, column?, search?, editor?}` | editor: explicit > project's own (.idea, .vscode) > `settings.desktop.editor` > first installed (VS Code, IDEA, PyCharm, WebStorm, PhpStorm, CLion, Studio, Zed, Kate, KWrite); argv per editor (`code <proj> -g f:l:c`, `kate -l l -c c f`, `idea <proj> --line l f`, `zed f:l:c`); returns location + 7 lines around it |
| `focus_context` | RO | `{include_clipboard_text?}` | "this file": active window (X11: title, class, pid, cwd, user files it holds open), clipboard file URIs (Dolphin Ctrl+C), clipboard text only as a char count unless asked, recently-used.xbel (existing files), newest downloads |
| `clipboard` | `get` RO; `set` MUT low, no approval | `{action: get\|set, text?}` | Klipper D-Bus |
| `input` | MUT medium, approval (rememberable per session) | `{action: type\|key\|click\|move\|scroll, text?, keys?, x?, y?, button?, double?, amount?}` | xdotool (X11); keys validated `^[A-Za-z0-9_]+(\+...)*$`, text max 2000 chars passed after `--`; error with install hint when xdotool is missing |
| `browser` | `status/tabs/read/screenshot` RO; `launch/open/navigate/activate/close/back/scroll` no approval; `click/type/key` approval (medium); `eval` always asks (high) | `{action, url?, tab?, element?, selector?, text?, submit?, key?, pixels?, script?, search?}` | Chrome DevTools Protocol on 127.0.0.1:`settings.browser.port` (9333) to KAgent's own profile `<data>/browser-profile` (Brave > Chrome > Chromium > Edge, or `settings.browser.binary`). `read` = title, URL, visible text (7000 chars), numbered interactive elements (`data-kagent` attribute); actions that change the page return a short snapshot; `search` on open/navigate types into the search box + Enter; URLs limited to http(s)/about:blank; page content is labelled untrusted |

## 16. Destructive actions (every mode)

Potentially destructive calls always show a confirmation, also with `full` control and
regardless of tool overrides or "always allow" rules; read-only mode denies them.
Covered: `system_power` reboot/poweroff, `process_signal` TERM/KILL/INT/HUP/QUIT,
`windows close`, `shell` / `terminal send` / `input type` whose text matches
`safety::looks_destructive`. Voice: the card is answered only by an explicit phrase
("confirm", "approve", "potrdi", or yes + the action word). Reboot/power off now is
refused within 10 minutes of boot.

## 15. KAgent's own UI (group `desktop`, 2026-11-24)

| Tool | Assess | Input | What |
|---|---|---|---|
| `ui` | `logs/state/snapshot` RO; others MUT low, no approval | `{action: logs\|state\|snapshot\|show\|voice_start\|voice_stop\|theme\|orb\|reload, errors_only?, view?, session?, value?}` | the web UI reports console errors/warnings, uncaught errors, unhandled rejections and React crashes (error boundary, with component stack) plus its state (tab id, view, voice status/mute/volume/transcript tail, approvals, theme and its choice `themePref`, size) to `POST /api/ui/report`; `theme` takes `dark`, `light` or `auto` (follow the system) every 2 s when changed, 10 s heartbeat; tabs silent for 30 s count as closed. `snapshot` asks open tabs for `document.body.innerText` over the event stream (`uiCommand`), answered on `POST /api/ui/snapshot/{id}` within 4 s. Commands never reach approvals. |
| `terminal` | `list` RO; `send` always asks (medium) | `{action: list\|send, target?, text?, enter?}` | Konsole tabs (D-Bus `org.kde.konsole-*/Sessions/N`: foreground program, cwd, title) and tmux panes; target = id or words matching program/folder/title ("claude KAgent"); ambiguous matches are errors. Konsole needs "Enable the security sensitive parts of the DBus API" (Configure Konsole -> General), else falls back to xdotool on that Konsole window. Never types sudo/pkexec/su/doas/run0 into a shell (clipboard instead). Enter is sent as a separate `\r` (TUIs like Claude Code) |
| `system_power` | reboot/poweroff always ask (high); suspend asks; lock/cancel no approval | `{action: reboot\|poweroff\|suspend\|lock\|cancel, minutes?}` | systemd-logind (`systemctl reboot [--when=+Nmin]`, `loginctl lock-session`), no root |
| `sessions` | `list/read` RO; `export` no approval (writes under home) | `{action: list\|read\|export, session?, path?}` | chat + voice history incl. tool calls as Markdown; export default `~/Documents/KAgent/<date>-<title>.md` |
| `screen` | asks (medium, rememberable): the image leaves the machine | `{target: window\|monitor\|desktop, question?}` | Spectacle (KDE) / gnome-screenshot; vision model over the OpenAI-compatible API: `settings.visionModel`, else chat/fallback model, else first enabled Gemini flash; on 429/503 the provider's other plain `gemini-N.N-flash` models newest first. **Live path (S46, `vision.md` section 3):** in a `live` or `auto` voice session the live model has `screen` whatever `voice.toolGroups` holds, with `{target?: window\|monitor\|desktop}` only (no `question`, no `window`); that call asks no vision model: it captures (the active window unless the owner asked for more), makes a JPEG of the longest side 1024, sends it to the live session as a `realtimeInput` video frame and answers `{"status": "ok", "output": "the screenshot is in your view now"}`. It is asked out loud once per voice session ("I want to look at your active window. Say yes to go ahead, or no."; the yes covers the later looks of the session). Never captured: a locked screen (`the screen is locked`) and KAgent's own approval cards (a call while a card waits is refused). A call that is not the live model's keeps the vision path. The screenshot file of a live look is deleted at once |
| `message` | asks (medium; "allow this session" covers the app) | `{app, text?, send?}` | xdotool: focus window by title, type (default not sent); send=true presses Enter as a separate command after 0.5 s; empty text + send = only Enter. `send` is enforced (agent-loop.md A.3): refused (`not sent: ...`) unless a message was typed in this session and the owner spoke or wrote since, and with text only when it is the text that was typed |
| `browser` `console` | RO | `{seconds?}` | console messages, exceptions and `Log` entries of the agent browser's tab (Chrome replays earlier ones on `Runtime.enable`/`Log.enable`) |

Tray (`kagentd/src/tray.rs`, StatusNotifierItem via ksni): left click = open the main
window, middle click = mute, scroll = voice volume; menu: Open KAgent, Orb widget
(always on top), Listen / Stop listening, Mute microphone, Voice volume 20-100%,
Alerts (n), Stop all tasks, Quit. App windows (`appwin.rs`): Chromium-family `--app`
windows with their own profiles (`<data>/app-profile`, `<data>/widget-profile`) and
classes `kagent-app` / `kagent-widget`; the widget gets keepAbove + all desktops +
skip taskbar (KWin script; wmctrl `above,sticky` on other X11 desktops). Disabled with
`KAGENT_NO_TRAY` (test scripts set it).

Voice: when a live session reaches `live`, playing MPRIS players are paused and resumed
when the session ends (`settings.voice.pauseMedia`, default true).

## 17. Skills (group `skills`; S18 core, S20 authoring, S21 sources)

Behaviour, roots and quarantine rules: `skills.md`. Tool input is snake_case, like the
other tools.

| Tool | Assess | Input | What |
|---|---|---|---|
| `load_skill` (updated in S18) | RO | `{name: string (required)}` | body of an **enabled** skill from the merged library (user > imported > bundled), frontmatter removed, `${CLAUDE_SKILL_DIR}` / `${ZCODE_SKILL_DIR}` / `${KAGENT_SKILL_DIR}` replaced by the skill's directory. Body capped at 100 KB. Counts one use in `<data>/skill-usage.json`. Unknown or disabled name -> error listing the enabled names grouped by source |
| `skill_save` (S20) | MUT, risk low, unprivileged, approval per mode (rememberable) | `{name, description: string (<= 1024), when_to_use?: string, body: string (<= 64 KB)}`. `body` may also come as an array of lines, joined with line breaks: the schema says `anyOf` string or array of strings. Reason (S20, measured on the raw Ollama 0.40.12 API with qwen2.5:7b-instruct, 6 runs each): with a `string` body the model's `skill_save` call arrived in 2 of 6 runs (1 of 6 in a second batch, 0 of 6 with a shorter description) and was otherwise generated and then dropped by the server; with an array of lines in 4, 5 and 6 of 6, with either form in 6 of 6 | writes `<config>/skills/<name>/SKILL.md` atomically (new, or overwrites a user skill). `name` `^[a-z0-9][a-z0-9-]{0,63}$`. Refuses reserved names (`skill-creator`, `widget-design`), names equal to an existing one except for case, an empty body, and a `<name>` folder that is a symlink or a file. The result says when the skill replaces one, hides one of a lower root (new or replaced), or is switched off in `skills.disabled`. Then the library is read again at once. `preview()` = new or replaced, name, description and the first 30 lines of the body |
| `skill_delete` (S20) | MUT, risk low, unprivileged, approval per mode | `{name: string}` | removes a **user** skill directory. Bundled and imported skills -> error saying where they come from; an unknown name -> error. The path must resolve inside `<config>/skills` and must not be a symlink before anything is deleted. The result says when a skill of a lower root is in use again. `preview()` = the folder and its files. Then the library is read again at once |

## 18. Memory (group `memory`; S25)

Store, recall and review rules: `memory.md`.

| Tool | Assess | Input | What |
|---|---|---|---|
| `memory_save` | MUT, risk low, unprivileged, approval per mode (rememberable) | `{name, description?: string, type: enum [user, feedback, machine, project, reference], content: string or an array of lines (<= 4000; the schema says `anyOf`, S20), replaces?: string}` | writes or replaces `<config>/memory/<name>.md` with frontmatter `name, description, type, created, updated, source: agent, reviewed: false` (the card says when it replaces a memory of the same name). `name` `^[a-z0-9][a-z0-9-]{0,63}$`. Empty description = first line of the content (<= 140 chars). `replaces` = name of the memory this one supersedes: that file gets `validUntil: today` and `supersededBy`. `preview()` = `Remember <name> (<type>): <first 400 chars>` |
| `memory_search` | RO | `{query: string, limit?: 1..10 (default 5)}` | lexical search with the recall scorer over all current memories -> `name (type, updated): description` + first 300 chars of the content |
| `memory_get` | RO | `{name: string}` | full content of one memory, or of a superseded one |
| `memory_forget` | MUT, risk low, no approval | `{name: string}` | marks the memory `validUntil: today` (kept on disk for the user, excluded from recall and search). Core files `user` and `machine` -> error "edit them in Settings -> Memory" |
| `session_search` | RO | `{query: string, limit?: 1..10 (default 5)}` | lexical search over the 200 most recently updated sessions (user and assistant text only, files over 5 MB skipped; the asking session is left out) -> `sessionId \| date \| title` + a snippet of at most 240 characters from the message with the most words of the query (of equals a reply, then the later one; a question asked in an earlier session gives the answer that followed it) |

With `memory.enabled=false` the five memory tools answer "memory is off"; the API and
the UI still read and edit the files.

## 19. Todo and plan (group `agent`; S23, S24)

| Tool | Assess | Input | What |
|---|---|---|---|
| `todo_write` | RO-equivalent (no approval; works in read-only and plan mode) | `{items: [{content: string (<= 200), status?: enum [pending, in_progress, completed], priority?: enum [high, medium, low]}], mode?: enum [replace, add]} (mode `add` puts the items after the ones already there; the default replaces the list; <= 50 items in the result; a missing status is pending; an item without content, content over 200 characters, an unknown field or an unknown status or priority refuses the whole list, which then stays as it was)` | replaces the session's list, normalised: at most one `in_progress` (the first; later ones become pending), missing priority = medium. Returns the list as `[1] [x] content` lines (a `>` marks in progress, a space pending, ` (high)` follows a high item; `no todo list` for an empty list). A replace that drops open items (by content) adds a paragraph naming them and pointing to mode `add`; an empty list (a clear) does not. Emits `todoUpdated` |
| `todo_read` | RO | `{}` | the list, or "no todo list" |
| `plan_enter` | RO-equivalent | `{reason?: string}` | puts the session in plan mode until a plan is approved or the owner cancels it (the flag is the session's): every call that is not read-only, and is not one of `todo_*`, `plan_*`, `load_skill`, `memory_search`, `memory_get`, `session_search`, `agent_run` (read-only profiles only), `agent_stop`, is refused with "plan mode: research only until the plan is approved". Already planning -> error. Emits `planUpdated` |
| `plan_exit` | RO-equivalent | `{plan: string (markdown, <= 12 KB); it may also come as an array of lines, joined with line breaks (the schema says `anyOf`)}` | raises an approval request with `kind: "plan"` (never rememberable). Approved -> plan mode off, the plan is saved under `<data>/plans/` and the result is "Plan approved. Carry it out step by step with the normal permissions." Denied with a reason -> "Plan rejected: <reason>. Revise the plan and call plan_exit again." Not planning, an empty plan, a plan over 12 KB or one that is neither text nor lines -> error, and no card. A rejection is a result, not an error. Emits `planUpdated` |

## 19a. Settings (group `agent`; S44)

Rules, protected keys and the voice side: `voice-control.md`.

| Tool | Assess | Input | What |
|---|---|---|---|
| `settings` | `get`, `models`: RO. `set`: MUT, risk medium, `always_ask`, never rememberable (a call that is refused for its keys is RO: it says why, without a card) | `{action: enum [get, models, set], key?: string (get), changes?: {<dotted key>: value} (set, 1 to 8 keys)}` | `get`: the settings as JSON (secrets `"<set>"` or `"<empty>"`), or the value at `key`. `models`: per provider its enabled models as `ref`, roles and the jobs they hold, and `voices` of the live model's template; no keys. `set`: one patch through the function of `PUT /api/settings`, all or none; result `{changed: [{key, old, new}], notes}`. Card: `change setting <key> from <old> to <new>` (joined with "; "). Protected keys, an unknown key (the 3 nearest are named) and a voice not of the template are errors |

## 20. Web (group `web`; S30)

Guards, extraction and engines: `web.md`. Both are read-only with side effect
`Network`; their output is labelled untrusted.

| Tool | Assess | Input | What |
|---|---|---|---|
| `web_fetch` | RO | `{url: http(s) URL, max_chars?: 1000..20000 (default 6000)}` | GET with the egress guard on every redirect (max 5), 15 s timeout, 2 MiB body cap, User-Agent `KAgent/<version>`. HTML -> title, main text, up to 30 links; `text/*`, JSON and XML -> raw text. Other types -> error naming the type and size. Cached 15 min (50 entries) |
| `web_search` | RO | `{query: string (<= 400), limit?: 1..10 (default 5)}` | results of the engine in `settings.search.engine` as `[n] title - url` + snippet. Engine `none` -> error "web search is off (Settings -> Web)". Engine failure -> error with the cause (rate limit, bad key, unreachable) |

Outside their range, `max_chars` and `limit` are read as the nearest end; a query over
400 characters is an error. An answer is at most 23 500 bytes: the links go first, then
the text (`web.md`).

## 21. Subagents and background tasks (group `agent`; S31)

Profiles, completion notifications and limits: `agents.md`.

| Tool | Assess | Input | What |
|---|---|---|---|
| `agent_run` | MUT, risk low, no approval | `{description: string (<= 80), prompt: string or an array of lines (the schema says `anyOf`), profile?: string (default "general"), model?: a model ref the providers know (`<providerId>::<modelId>`), else an error and no subagent, background?: boolean (default true)}` | starts a child session (`origin: subagent`, parent = this session) running the prompt with the profile's model and tool groups. Returns `agentId` and `running` or `queued`. `background: false` waits up to 120 s, then keeps running in the background and returns. Because it is MUT, a planning session cannot start a subagent |
| `agent_wait` | RO | `{ids: string[] (<= 8), timeout_sec?: 0..120 (default 60)}` | output (<= 6000 chars each) and state of the named agents when they finish or the time is up |
| `agent_stop` | MUT, risk low, no approval | `{id: string}` | cancels the child's running turn |
| `agent_list` | RO | `{}` | children of this session: id, profile, state, duration |
| `agent_tell` | MUT, risk low, `always_ask` | `{id?: string, name?: part of the name of its job, text: string (<= 2000), interrupt?: boolean (default false)}` | the owner's message to a running child of this session (`steering.md`): read at its next model request as `[from the owner] ...` (it keeps going), or with `interrupt` after its current request or command is cancelled, and the session resumes with the message first. "Told \"<job>\": it will see it at its next step." Only the main agent has it: a subagent is refused (depth rule), another session's child is `not_done`. Never an approval |
| `task_output` | RO | `{task_id: string, wait_sec?: 0..30 (default 0), tail?: 100..20000 (default 4000)}` | state and the last `tail` characters of a background shell task |
| `task_stop` | MUT, risk low, no approval | `{task_id: string}` | kills the task's process group |

## 22. Widgets (group `desktop`; S33)

| Tool | Assess | Input | What |
|---|---|---|---|
| `widget_show` | MUT, risk low, no approval | `{title: string (<= 80), html: string (<= 24 KB; it may also come as an array of lines, joined with line breaks: the schema says `anyOf` string or array of strings, S20)}` | saves `<data>/widgets/<sessionId>/<n>.html` with its title in `<n>.title` (the title is shown above the panel), refuses `html` over 24 KB, empty or of another form, and returns the marker `::widget{id="<sessionId>/<n>"}` to place on its own line in the answer. Session `mcp` -> error "widgets need a chat session". Rendering and sandbox: `widgets.md` |

## 23. Changes to existing tools

| Tool | Stage | Change |
|---|---|---|
| `tasks` | S28 | the schedule argument is `schedule` (was `when`) and also accepts 5-field cron and "every N hours/days/weeks/months"; `what` is `anyOf` string or array of lines; new per-task fields `deliver`, `on_approval` (`deny`\|`wait`), `session` (`new`\|`reuse`), `timeout_min` (1..1440); `delay_minutes` (1..525600); new actions `cancel` and `history`; a key that is not a field is refused. `automations.md` |
| `shell` | S31 | `background: boolean`: returns `{taskId, outputFile}` at once; output file `<data>/task-out/<taskId>.log` (cap 1 MiB), at most 20 running, killed by stop-all and at daemon exit |
| `browser` | S27 | acts on the backend from `settings.browser.backend`; attach-mode tab rules in `browser.md`; a call judged for one of the owner's tabs has the rule key `browser@owner <action>` and a summary ending in `(the owner's tab)` |
| `ui` | S19 | `show` gains `section?: string` (settings section id). Views `providers` and `tools` now open the settings sections `models` and `tools` |
| `load_skill` | S18 | section 17 |
| `agent_wait`, `agent_stop`, `agent_list` | S35 | also wait for, stop and list the coder runs of the session: they are children of the same registry (`coders.md` section 2), and the answers have the shape of section 21 |

## 24. Voice-only function `think` (S26)

Not in the tool registry: text turns and MCP never see it. It is declared to the live
model only when `voice.brain` is `routed` or `auto` (`voice.md`).

```
name: think
description: Hand the user's words to the brain (the main agent with memory, skills
  and tools) and get back the text to say aloud.
parameters: {text: string (required): the user's exact words, in their language}
```
Result: `{"reply": string, "status": "ok" | "failed" | "cancelled" | "handled"}`.
`handled` = the utterance answered an approval card; `reply` is empty then. On `/live`
the call shows as the tool `think` (voice.md section 2).

## 25. Coding agents (group `coders`; S35, S37)

KAgent starts Claude Code or Codex as a child process for one job in one project
folder. The run is a child of the session like a subagent (section 21): the same queue,
states, events, card and report. Runs, profiles, the report and what a coder may do:
`coders.md`.

| Tool | Assess | Input | What |
|---|---|---|---|
| `coder_run` | by the profile's mode: `plan` MUT, risk low, no approval (as `agent_run`); `edit` MUT, risk medium, rule key `coder_run <profile> <project>`; `full` always asks (risk high), never rememberable. An unknown profile assesses as `full`. A prompt that `looks_destructive` is destructive | `{description: string (<= 80), prompt: string or an array of lines (the schema says `anyOf`; <= 16000 chars), project: string (a project name, as the `projects` tool lists it), profile?: string (default `settings.coders.defaultProfile`), background?: boolean (default true); S37 adds `resume?: string`; S38 adds `worktree?: boolean}` | starts a coder run (a child session with origin `subagent`, parent = this session) with the job on the tool's stdin. Returns `agentId` and `running` or `queued`; with `background: false` it waits up to 120 s for the report, then answers as `agent_run` does. The report arrives later as a message that starts with `[Agent ...]` (`coders.md` section 2). Read-only mode and plan mode refuse the call; a subagent does not have it |
| `coder_sessions` (S37) | RO | `{action: list\|read\|worktrees, project?, tool?, session?, limit?: 1..50 (default 20)}` | the conversations Claude Code and Codex kept for a project on this machine, the last messages of one, and the linked worktrees of a project (`coders.md` sections 7 and 8). The text of a conversation is labelled untrusted |

The description of `coder_run` tells the model: it is for code in a project folder
(implementing, fixing, refactoring, tests, review), not for this machine's files,
settings or apps, and not for research (the agent's own tools, `agent_run`); `prompt`
holds the whole job; `profile` says what the coder may do (`claude-plan` reads and
analyses, `claude-edit` edits and runs tests, `claude-full` anything with the user's
approval; S36 adds the `codex-*` names); for N jobs it makes N calls in one answer. The
wording is settled with raw replays of the local model (`coders.md` section 4).

## 26. Calendar (group `calendar`; S45)

Google Calendar and places, a native module of the daemon (`calendar.md`). The group is not
in `DEFAULT_TOOL_GROUPS`: the first successful connection adds it to `toolGroups` and to
`voice.toolGroups` ("default on once connected"); a later connection leaves the owner's
choices alone. Until then every tool answers that the calendar is switched off or not
connected, and says where to fix it. Results that hold calendar text (titles, places,
guests) start with `[untrusted: calendar data ...]`.

| Tool | Assess | Input | What |
|---|---|---|---|
| `calendar_events` | RO, side effect Network | `{from?: date or datetime, to?: date or datetime, query?: string, calendar?: name or id, max?: 1..50 (default 25)}` | the events of the switched-on calendars in the range (default: today; a date as `to` includes that day; at most 366 days), sorted by start: when (machine time zone, with the weekday), title, place, calendar, id, Meet link, guests, `overlaps`, "one event of a series". A calendar that is not switched on is refused; Google failing is an error, never an empty list |
| `calendar_change` | MUT, risk medium, `always_ask`; `delete` is destructive | `{action: add\|update\|delete, id?, calendar?, title?, start?, end?, durationMin?, allDay?, location?: {name, address, mapsUrl?}, notes?, reminderMin?: integer\|null, meet?: boolean, guests?: [email]}` | `add` needs title and start, then `end` or `durationMin` (default `calendar.defaultDurationMin`); `update` and `delete` need the `id` of `calendar_events`. The card and the spoken question are built from the event as Google stores it (`Tool::prime` reads it first), never from the model's words: `add "Lunch with Miha", Sat 28 Nov 12:00 to 13:00, at Thai Orchid, Glavni trg 5, Maribor, calendar Personal, reminder 30 min` / `move "<title>" from <old> to <new>` / `delete "<title>", <when>`. Refusals, before anything is sent: a calendar that is not switched on or is read-only; `end` not after `start`; a start more than 5 years from now; more than 20 guests or a guest that is not an email address; `mapsUrl` that is not a Google Maps link; a whole series (`recurrence`: "change the series in Google Calendar"); an `id` not found in a switched-on calendar; nothing to change. Guests get Google's mail only when `guests` is given (only the new ones on `update`); a delete tells nobody. Journaled with its way back: `add` -> delete, `update` -> the old fields written back, `delete` -> the stored copy inserted as a new event (Google keeps the old id; a Meet link is lost) |
| `place_search` | RO, side effect Network | `{query: string (<= 200), near?: string}` | up to 3 candidates (name, address, `mapsUrl`, `source`): Google Places (New) with a Maps key, biased to `near` or to `calendar.home`; without a key, or when Places fails or finds nothing, the web search, the address taken from the first results that have one, `source: "web"`, and the answer says "I could not check this on the map" |
| `calendar_report` | RO, side effect Network | `{day?: today\|tomorrow}` | the morning report's text for the model (`calendar.md` section 6): changes nothing and sends nothing |

The permission rules are those of `voice-control.md` section 4: `add` and `update` ask in
`ask` and `trusted`, ask from voice in every mode, and pass without a card in chat under
`full`; `delete` asks in every mode and by voice only "confirm" answers it.

## 27. Local models (group `agent`; S47)

| Tool | Assess | Input | What |
|---|---|---|---|
| `local_models` | RO | `{}` | text for the model: whether Ollama answers (and its version), the graphics card with its free memory (or "no graphics card"), RAM, and the shipped list of recommended local models per job with a fit verdict (`fits`, `tight`, `too big`, `slow`), which one is RECOMMENDED, which are installed, and which cannot be used yet (speech to text). It never downloads: it tells the model that the owner presses Download in Settings -> Local models (`local-models.md` section 5) |

Registered by the daemon like `calendar_events` (it finds the module once it exists) and listed in
`group_of` with `settings`.

## 28. Screen watch (group `desktop`; S48)

| Tool | Assess | Input | What |
|---|---|---|---|
| `screen_watch` `start` | MUT, `always_ask`, side effect none, risk medium | `{action: "start", goal: string (<= 200), minutes?: 1..120 (default `screenWatch.defaultMinutes`), everySec?: 5..60 (default `screenWatch.everySec`), mode?: "once"\|"repeat", target?: "window"\|"monitor"}` | Starts the one watch (`screen-watch.md` sections 1 and 2). Summary: `watch "<title of the active window>" for <n> minutes: <goal>`, or `watch the monitor for ...`. It asks every time, in every permission mode, and from voice it is asked out loud like any `always_ask` call (`voice-control.md` section 4); no stored rule satisfies it |
| `screen_watch` `status` | RO | `{action: "status"}` | The running watch in words (what it looks at, until when, the interval, looks so far, the last note, paused), else how the last one ended, else "no screen watch" |
| `screen_watch` `stop` | RO-equivalent (`as_read_only`) | `{action: "stop"}` | Ends it as `stopped`; "no screen watch is running" when none runs. The daemon also catches "stop watching" said in a live session |

Refusals are errors that say what to do: a second `start` names the running goal; no local vision model names Settings ->
Local models and the model it recommends; `screenWatch.enabled` off names the card; a goal over 200 characters, minutes outside
1..120 or `everySec` outside 5..60; an active window that is KAgent itself ("click the window to watch first, or use target
monitor"). Registered by the daemon like `local_models` (a cell the watch fills); `group_of` maps it to `desktop`.

## 29. Usage (group `agent`; v0.0.3)

| Tool | Assess | Input | What |
|---|---|---|---|
| `usage` | RO | `{provider?: string}` (an id or a name, any case) | text for the model per provider: each limit as "Requests per day 47 of 50 (94 %, nearly out), resets 09:00." (a limit that is KAgent's own count says "counted by KAgent: other apps using this key are not included"; one known only to be out has no numbers), "No limits: a local provider." or "No limit known: ...", today's and this month's requests and tokens through KAgent, and the busiest models. An unknown provider is `not_done("no provider named x", "call usage without a provider to list them (providers: ...)")` (`agent-loop.md` section B). Live voice is not counted (`usage.md` section 6) |

Registered by the daemon like `local_models` (it finds the usage module once it exists) and listed in `group_of` with `settings`.

## 30. Tickets (group `agent`; v0.0.3)

| Tool | Assess | Input | What |
|---|---|---|---|
| `tickets` | `list` RO; `add`, `update`, `close` MUT, risk low, `always_ask` (a card in chat in every mode but `full`; out loud from voice in every mode) | `{action: "add"\|"update"\|"list"\|"close", key?, title?, description?, state?, priority?, labels?, done_when?, parent?, note?, text?, label?, all_projects?}` | the owner's tickets of the calling session's project (a subagent: its parent's), else the Inbox. `add` "Added KA-14 \"Title\" (todo, medium) to kagent."; `update` "KA-14 updated: state: todo -> in_progress."; `close` "KA-14 is done."; `list` at most 30 lines `KA-14 [in_progress, high] Title (labels)` and "(N more: ...)"; refusals are `not_done` (`agent-loop.md` section B) |

Details and the prompt rule ("Add to the todo ... makes a ticket; `todo_write` is only the working list of a multi-step job"):
`tickets.md` section 5. Registered by the daemon like `usage` (a cell the store fills) and listed in `group_of` with `settings`.

`agent_tell` (section 21, `steering.md` section 5) is the subagent tool added in the same item.

## 31. Workflow runs (group `agent`; v0.0.3)

| Tool | Assess | Input | What |
|---|---|---|---|
| `workflow_run` | MUT, risk medium, `always_ask` (the summary names the workflow, the project and the workers) | `{workflow, goal, project?, ticket?}` | starts a run (`workflows.md`), the calling session answers its workers' questions; "Started run run_1a2b3c4d \"title\" (plan-only): planning." |
| `workflow_status` | RO | `{run?}` | one run with its tasks and open questions, or the runs of the calling session's project |
| `workflow_stop` | MUT, risk low, `always_ask` | `{run}` | stops a run |
| `workflow_answer` | MUT, no approval (it answers a worker, it approves nothing) | `{run, question, text?, ask_owner?}` | the answer reaches the worker; a text that starts like the owner's or a coworker's message is refused; `ask_owner` turns the question into a card |
| `report_progress` | no approval | `{steps: [{title, status}]}` | a worker's own steps, shown under its task |
| `ask` | no approval | `{question, options?, blocking?}` | a worker's question (`blocking` default true: the answer is the result, up to 30 minutes) |

`report_progress` and `ask` are offered only to a worker of a run and refused at execution to every other session ("... is for a worker
of a workflow run"); a read-only worker (planner, reviewer) may call them. A subagent has none of the four `workflow_*` tools (the depth
rule). Registered by the daemon through `WorkflowAccess` (a handle the engine fills) and listed in `group_of` with `settings`.
