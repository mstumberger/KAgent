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
- **Missing program** (e.g. `nvidia-smi` absent): return `ok` with `unavailable:
  <program> not installed` for that section, never fail the whole tool.
- **Per-call assessment** (`assess`): fields `read_only, needs_approval, always_ask,
  privileged, concurrency_safe, risk, summary, rule_key`. Columns below:
  RO = `CallAssessment::as_read_only()`; MUT = mutating (needs approval, not
  concurrency safe); PRIV = privileged (always asks, never satisfied by a rule).
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
| `ui` | `logs/state/snapshot` RO; others MUT low, no approval | `{action: logs\|state\|snapshot\|show\|voice_start\|voice_stop\|theme\|orb\|reload, errors_only?, view?, session?, value?}` | the web UI reports console errors/warnings, uncaught errors, unhandled rejections and React crashes (error boundary, with component stack) plus its state (tab id, view, voice status/mute/volume/transcript tail, approvals, theme, size) to `POST /api/ui/report` every 2 s when changed, 10 s heartbeat; tabs silent for 30 s count as closed. `snapshot` asks open tabs for `document.body.innerText` over the event stream (`uiCommand`), answered on `POST /api/ui/snapshot/{id}` within 4 s. Commands never reach approvals. |
| `terminal` | `list` RO; `send` always asks (medium) | `{action: list\|send, target?, text?, enter?}` | Konsole tabs (D-Bus `org.kde.konsole-*/Sessions/N`: foreground program, cwd, title) and tmux panes; target = id or words matching program/folder/title ("claude KAgent"); ambiguous matches are errors. Konsole needs "Enable the security sensitive parts of the DBus API" (Configure Konsole -> General), else falls back to xdotool on that Konsole window. Never types sudo/pkexec/su/doas/run0 into a shell (clipboard instead). Enter is sent as a separate `\r` (TUIs like Claude Code) |
| `system_power` | reboot/poweroff always ask (high); suspend asks; lock/cancel no approval | `{action: reboot\|poweroff\|suspend\|lock\|cancel, minutes?}` | systemd-logind (`systemctl reboot [--when=+Nmin]`, `loginctl lock-session`), no root |
| `sessions` | `list/read` RO; `export` no approval (writes under home) | `{action: list\|read\|export, session?, path?}` | chat + voice history incl. tool calls as Markdown; export default `~/Documents/KAgent/<date>-<title>.md` |
| `screen` | asks (medium, rememberable): the image leaves the machine | `{target: window\|monitor\|desktop, question?}` | Spectacle (KDE) / gnome-screenshot; vision model over the OpenAI-compatible API: `settings.visionModel`, else chat/fallback model, else first enabled Gemini flash; on 429/503 the provider's other plain `gemini-N.N-flash` models newest first |
| `message` | asks (medium; "allow this session" covers the app) | `{app, text?, send?}` | xdotool: focus window by title, type (default not sent); send=true presses Enter as a separate command after 0.5 s; empty text + send = only Enter |
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
| `skill_save` (S20) | MUT, risk low, unprivileged, approval per mode (rememberable) | `{name, description: string (<= 1024), when_to_use?: string, body: string (<= 64 KB)}` | writes `<config>/skills/<name>/SKILL.md` atomically (new, or overwrites a user skill). `name` `^[a-z0-9][a-z0-9-]{0,63}$`. Refuses reserved names (`skill-creator`, `widget-design`) and names equal to an existing one except for case. Overwriting a user skill that shadows a bundled skill says so in the result. `preview()` = name, description and the first 30 lines of the body |
| `skill_delete` (S20) | MUT, risk low, unprivileged, approval per mode | `{name: string}` | removes a **user** skill directory. Bundled and imported skills -> error saying where they come from. The path must resolve inside `<config>/skills` and must not be a symlink before anything is deleted |

## 18. Memory (group `memory`; S25)

Store, recall and review rules: `memory.md`.

| Tool | Assess | Input | What |
|---|---|---|---|
| `memory_save` | MUT, risk low, unprivileged, approval per mode (rememberable) | `{name, description?: string, type: enum [user, feedback, machine, project, reference], content: string (<= 4000), replaces?: string}` | writes `<config>/memory/<name>.md` with frontmatter `name, description, type, created, updated, source: agent, reviewed: false`. `name` `^[a-z0-9][a-z0-9-]{0,63}$`. Empty description = first line of the content (<= 140 chars). `replaces` = name of the memory this one supersedes: that file gets `validUntil: today` and `supersededBy`. `preview()` = `Remember <name> (<type>): <first 400 chars>` |
| `memory_search` | RO | `{query: string, limit?: 1..10 (default 5)}` | lexical search with the recall scorer over all current memories -> `name (type, updated): description` + first 300 chars of the content. `memory.enabled=false` -> error "memory is off" |
| `memory_get` | RO | `{name: string}` | full content of one memory, or of a superseded one |
| `memory_forget` | MUT, risk low, no approval | `{name: string}` | marks the memory `validUntil: today` (kept on disk for the user, excluded from recall and search). Core files `user` and `machine` -> error "edit them in Settings -> Memory" |
| `session_search` | RO | `{query: string, limit?: 1..10 (default 5)}` | lexical search over the 200 most recently updated sessions (user and assistant text only, files > 5 MB skipped) -> `sessionId \| date \| title` + a 240-char snippet around the best match |

## 19. Todo and plan (group `agent`; S23, S24)

| Tool | Assess | Input | What |
|---|---|---|---|
| `todo_write` | RO-equivalent (no approval; works in read-only and plan mode) | `{items: [{content: string (<= 200), status?: enum [pending, in_progress, completed], priority?: enum [high, medium, low]}]} (<= 50 items)` | replaces the session's list, normalised: at most one `in_progress` (the first; later ones become pending), missing priority = medium. Returns the list as `[1] [x] content` lines. Emits `todoUpdated` |
| `todo_read` | RO | `{}` | the list, or "no todo list" |
| `plan_enter` | RO-equivalent | `{reason?: string}` | puts the session in plan mode for the rest of the turn: every call that is not read-only, and is not one of `todo_*`, `plan_*`, `load_skill`, `memory_search`, `memory_get`, `session_search`, is refused with "plan mode: research only until the plan is approved". Already planning -> error. Emits `planUpdated` |
| `plan_exit` | RO-equivalent | `{plan: string (markdown, <= 12 KB)}` | raises an approval request with `kind: "plan"` (never rememberable). Approved -> plan mode off, the plan is saved under `<data>/plans/` and the result is "Plan approved. Carry it out step by step with the normal permissions." Denied with a reason -> "Plan rejected: <reason>. Revise the plan and call plan_exit again." Not planning, or an empty plan -> error. Emits `planUpdated` |

## 20. Web (group `web`; S30)

Guards, extraction and engines: `web.md`. Both are read-only with side effect
`Network`; their output is labelled untrusted.

| Tool | Assess | Input | What |
|---|---|---|---|
| `web_fetch` | RO | `{url: http(s) URL, max_chars?: 1000..20000 (default 6000)}` | GET with the egress guard on every redirect (max 5), 15 s timeout, 2 MiB body cap, User-Agent `KAgent/<version>`. HTML -> title, main text, up to 30 links; `text/*`, JSON and XML -> raw text. Other types -> error naming the type and size. Cached 15 min (50 entries) |
| `web_search` | RO | `{query: string (<= 400), limit?: 1..10 (default 5)}` | results of the engine in `settings.search.engine` as `[n] title - url` + snippet. Engine `none` -> error "web search is off (Settings -> Web)". Engine failure -> error with the cause (rate limit, bad key, unreachable) |

## 21. Subagents and background tasks (group `agent`; S31)

Profiles, completion notifications and limits: `agents.md`.

| Tool | Assess | Input | What |
|---|---|---|---|
| `agent_run` | MUT, risk low, no approval | `{description: string (<= 80), prompt: string, profile?: string (default "general"), model?: model ref, background?: boolean (default true)}` | starts a child session (`origin: subagent`, parent = this session) running the prompt with the profile's model and tool groups. Returns `agentId` and `running` or `queued`. `background: false` waits up to 120 s, then keeps running in the background and returns |
| `agent_wait` | RO | `{ids: string[] (<= 8), timeout_sec?: 0..120 (default 60)}` | output (<= 6000 chars each) and state of the named agents when they finish or the time is up |
| `agent_stop` | MUT, risk low, no approval | `{id: string}` | cancels the child's running turn |
| `agent_list` | RO | `{}` | children of this session: id, profile, state, duration |
| `task_output` | RO | `{task_id: string, wait_sec?: 0..30 (default 0), tail?: 100..20000 (default 4000)}` | state and the last `tail` characters of a background shell task |
| `task_stop` | MUT, risk low, no approval | `{task_id: string}` | kills the task's process group |

## 22. Widgets (group `desktop`; S33)

| Tool | Assess | Input | What |
|---|---|---|---|
| `widget_show` | MUT, risk low, no approval | `{title: string (<= 80), html: string (<= 24 KB)}` | saves `<data>/widgets/<sessionId>/<n>.html` and returns the marker `::widget{id="<sessionId>/<n>"}` to place on its own line in the answer. Session `mcp` -> error "widgets need a chat session". Rendering and sandbox: `widgets.md` |

## 23. Changes to existing tools

| Tool | Stage | Change |
|---|---|---|
| `tasks` | S28 | `schedule` also accepts 5-field cron and "every N hours/days/weeks/months"; new per-task fields `deliver`, `on_approval` (`deny`\|`wait`), `session` (`new`\|`reuse`), `timeout_min`; new actions `cancel` and `history`. `automations.md` |
| `shell` | S31 | `background: boolean`: returns `{taskId, outputFile}` at once; output file `<data>/task-out/<taskId>.log` (cap 1 MiB), at most 20 running, killed by stop-all and at daemon exit |
| `browser` | S27 | acts on the backend from `settings.browser.backend`; attach-mode tab rules in `browser.md` |
| `ui` | S19 | `show` gains `section?: string` (settings section id). Views `providers` and `tools` now open the settings sections `models` and `tools` |
| `load_skill` | S18 | section 17 |

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
`handled` = the utterance answered an approval card; `reply` is empty then.
