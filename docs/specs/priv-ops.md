# Spec: root helper `kagent-priv`

The only program that runs as root. Security-critical: keep it small, synchronous,
dependency-light (`serde`, `serde_json`, `regex`, `sha2`), no network, no shell.

## 1. Invocation

```
sudo -n /usr/local/libexec/kagent-priv            # reads ONE JSON op from stdin
```
- Sudoers drop-in `/etc/sudoers.d/kagent`: `<user> ALL=(root) NOPASSWD: /usr/local/libexec/kagent-priv`
  (exact path, no arguments allowed). Installed by the owner from `kagentd sudoers`.
- stdin max 1 MiB; reject otherwise. stdout: exactly one JSON response line. stderr:
  nothing (all diagnostics go into the response). Exit code 0 when the response was
  written (even for `ok:false`), 2 on unparsable input.
- `SUDO_UID` must be set and match the uid that owns `$HOME` paths in requests; the
  helper never trusts `HOME` from the environment (resolve from `getpwuid(SUDO_UID)`).
- `pkexec` mode: same binary via `pkexec /usr/local/libexec/kagent-priv`
  (uses `PKEXEC_UID` instead of `SUDO_UID`).
- Environment for child commands: cleared, then `PATH=/usr/sbin:/usr/bin:/sbin:/bin`,
  `LC_ALL=C.UTF-8`. Children run with absolute program paths, argv arrays, never a shell.
- Timeouts per op (below); on timeout kill the child and return `code:"timeout"`.

## 2. Request / response

Request: `{"op": "<name>", ...fields}` plus optional `"dryRun": true` (validate and
compute undo, do not execute).

Response:
```json
{"ok": true, "op": "sysctl_set", "stdout": "...", "stderr": "...", "exit": 0,
 "undo": {"op": "sysctl_set", "key": "vm.swappiness", "value": "60", "persist": true},
 "before": "60", "after": "10"}
{"ok": false, "op": "sysctl_set", "code": "invalid_input|not_allowed|failed|timeout|unsupported", "message": "..."}
```
stdout/stderr capped at 64 KiB each (tail kept).

## 3. Operations

| op | Fields | Validation | Executes | Undo | Timeout |
|---|---|---|---|---|---|
| `sysctl_set` | `key, value, persist` (default true) | key `^[a-z0-9_]+(\.[a-z0-9_-]+){1,6}$`, must exist under `/proc/sys`; value `^[A-Za-z0-9 _.,:/-]{1,256}$`; deny keys matching `^(kernel\.(modules_disabled|kexec_load_disabled|core_pattern)|kernel\.yama\..*)$` | `/usr/sbin/sysctl -w key=value`; persist: rewrite `/etc/sysctl.d/99-kagent.conf` (replace or append `key = value` line) | old value from `/proc/sys`, same persist flag (and restore previous file content) | 10 s |
| `modprobe` | `module, remove, params{}` | module `^[A-Za-z0-9_-]{1,64}$`; param keys `^[A-Za-z0-9_]{1,64}$`, values `^[A-Za-z0-9_.,:/=-]{0,128}$`; deny modules listed in `deny_modules` (empty by default) | `/usr/sbin/modprobe [-r] module [k=v...]` | load <-> remove (only if state changed) | 30 s |
| `modprobe_conf` | `module, params{}` (empty = delete file) | as above | write `/etc/modprobe.d/kagent-<module>.conf` with `options <module> k=v ...` | previous file content or delete | 5 s |
| `systemctl` | `action, unit` | action in `start stop restart reload enable disable`; unit `^[A-Za-z0-9@_.:-]{1,128}$`; deny stop/disable for `^(dbus|systemd-.*|polkit|ssh|sshd|kagent.*)\.service$` | `/usr/bin/systemctl <action> <unit>` | from `is-active` / `is-enabled` before: start<->stop, enable<->disable; restart/reload: no undo | 60 s |
| `package` (alias `apt`) | `action, packages[], simulate` | action `install|remove`; names `^[A-Za-z0-9][A-Za-z0-9+._@-]{0,99}$`, 1-200 (the agent sends at most 20; undo lists carry dependencies); package manager detected by the helper from /etc/os-release + binaries, never chosen by the caller | dry run first, abort if it fails (dnf: by output text, its exit code is meaningless). apt: `/usr/bin/apt-get -y [-s] install|remove -- <pkgs>`; dnf: `/usr/bin/dnf --assumeno` then `-y install|remove -- <pkgs>`; pacman: `/usr/bin/pacman -S --needed --print` / `-R --print`, then `-S --noconfirm --needed` / `-R --noconfirm` | apt: from the simulate run's `Inst`/`Remv` lines; dnf/pacman: diff of installed names (`rpm -qa --qf %{NAME}`, `pacman -Qq`) before/after. install -> remove what was added, remove -> install what was removed | 600 s |
| `write_file` | `path, content, mode` | path in allowlist (below), content <= 256 KiB, mode `^0[0-7]{3}$` | atomic write (tmp + rename), chown root:root | previous content (or delete if new); backup copy under `/var/lib/kagent/backup/<ts>-<sanitized path>` | 5 s |
| `read_root` | `path, tail_lines` | path in read allowlist (below) | read last N lines (max 2000) | none | 5 s |
| `kill` | `pid, signal` | pid > 1, signal in `TERM KILL HUP INT STOP CONT USR1`; deny pid of PID 1 children named `systemd`, and kernel threads (empty cmdline) | `kill(2)` | none | 2 s |
| `smartctl` | `device` | `^/dev/(sd[a-z]{1,2}|nvme[0-9]+n[0-9]+|hd[a-z])$` | `/usr/sbin/smartctl -a -j <device>` | none | 30 s |
| `firewall_status` (alias `ufw_status`) | none | | `/usr/sbin/ufw status verbose`, else `/usr/bin/firewall-cmd --list-all`, else `/usr/sbin/nft list ruleset` (400 lines) | none | 15 s |
| `bpftrace` | `script, seconds, pathPrefix?` | script id in table section 4; seconds 1-10; pathPrefix `^/[A-Za-z0-9._/-]{1,200}$` (open-snoop only) | `/usr/bin/bpftrace -e <rendered script>` | none | seconds + 15 s |
| `shell_root` | `command` | only when `/etc/kagent/allow-root-shell` exists (owner creates it manually); command <= 4 KiB | `/bin/bash -c <command>` | none | 120 s |

**write_file allowlist** (canonicalized, no symlinks in the final component):
`/etc/sysctl.d/99-kagent.conf`, `/etc/modprobe.d/kagent-*.conf`,
`/etc/systemd/system/*.d/kagent-*.conf`, `/etc/systemd/journald.conf.d/kagent-*.conf`,
`/etc/NetworkManager/conf.d/kagent-*.conf`, `/etc/udev/rules.d/90-kagent-*.rules`.

**read allowlist**: `/var/log/auth.log`, `/var/log/syslog`, `/var/log/kern.log`,
`/var/log/apt/history.log`, `/var/log/dpkg.log`, `/var/log/ufw.log`, Fedora
`/var/log/secure`, `/var/log/messages`, `/var/log/dnf*.log`, `/var/log/firewalld`, Arch
`/var/log/pacman.log`,
`/etc/sudoers`, `/etc/sudoers.d/*`, `/etc/shadow` is **never** allowed.

## 4. Embedded bpftrace scripts (Ubuntu 26.04 bpftrace, `args.field` syntax)

`{S}` = validated seconds, `{P}` = validated prefix, `{N}` = byte length of prefix.
Render by string replacement of these placeholders only.

```
io-by-process:
tracepoint:block:block_rq_issue { @bytes[comm] = sum(args.bytes); }
interval:s:{S} { exit(); }

runq-latency:
tracepoint:sched:sched_wakeup, tracepoint:sched:sched_wakeup_new { @q[args.pid] = nsecs; }
tracepoint:sched:sched_switch /@q[args.next_pid]/ {
  @usecs = hist((nsecs - @q[args.next_pid]) / 1000); delete(@q[args.next_pid]); }
interval:s:{S} { clear(@q); exit(); }

syscalls-top:
tracepoint:raw_syscalls:sys_enter { @[comm] = count(); }
interval:s:{S} { exit(); }

tcp-retransmits:
tracepoint:tcp:tcp_retransmit_skb { @[ntop(args.daddr), args.dport] = count(); }
interval:s:{S} { exit(); }

exec-snoop:
tracepoint:syscalls:sys_enter_execve {
  printf("%d %d %s %s\n", pid, curtask->real_parent->tgid, comm, str(args.filename)); }
interval:s:{S} { exit(); }

open-snoop:
tracepoint:syscalls:sys_enter_openat /strncmp(str(args.filename), "{P}", {N}) == 0/ {
  printf("%d %d %s %s\n", pid, curtask->real_parent->tgid, comm, str(args.filename)); }
interval:s:{S} { exit(); }
```
Stage S16 must run each once on the machine; if a field name differs in the installed
bpftrace (`bpftrace -lv <tracepoint>`), fix the script here and in the code and log it
in IMPLEMENTATION.md section E. For `tcp-retransmits`, fallback `@[args.dport] = count();`.

Long-running variant for S14 honeytoken attribution: op `bpftrace_stream` with script
`open-snoop`, `seconds` = 0 meaning "until stdin closes", output streamed line by line.
Only paths registered as honeytokens (helper re-validates the prefix is under the
requesting user's home).

## 5. Tests (no root needed)

- Validation table: >= 30 cases (valid/invalid per op, path traversal `../`, symlink
  target outside allowlist, deny-listed units and sysctl keys, oversized input).
- Rendering of each bpftrace script with placeholders.
- `dryRun` returns the computed undo without executing (executor behind a trait, faked
  in tests).
