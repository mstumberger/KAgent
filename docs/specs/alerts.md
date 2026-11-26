# Spec: watchers, alerts, intrusion detection, traffic rules

## 1. Alert model

```json
{"id": "alr_20261123_100000_ab12", "ts": "...", "lastSeen": "...", "count": 3,
 "rule": "ssh-bruteforce", "severity": "info|review|high",
 "status": "open|acked|resolved",
 "title": "12 failed SSH logins from 203.0.113.9 in 5 min",
 "cause": "repeated password failures for invalid users",
 "evidence": [{"kind": "log|process|file|flow|router", "text": "...", "data": {}}],
 "proposal": "block 203.0.113.9 on the router for 1 day (router_block_ip)",
 "dedupeKey": "ssh-bruteforce:203.0.113.9"}
```
- Same `dedupeKey` within the rule's cooldown -> increment `count`, update `lastSeen`,
  emit `alertUpdated` (no new notification unless severity rises).
- `proposal` text: rules give a static template; for `review`/`high` the daemon may
  replace it with a fast-model suggestion (single call, evidence only, 15 s timeout,
  static template on failure). The model never acts from a watcher; actions happen
  only in a session the owner starts (`/alerts/:id/investigate`).
- Notification: desktop notification for `review` (normal urgency) and `high`
  (critical urgency); `info` only in the UI. Quiet hours suppress `info`/`review`
  notifications (stored, shown later), never `high`. Spoken only if
  `watchers.spoken` and a live session is active.
- Every raised alert is written to the audit log (`kind: alert`).

## 2. Watcher framework (S13)

```rust
#[async_trait] pub trait Watcher: Send + Sync {
    fn name(&self) -> &'static str;
    async fn run(&self, tx: mpsc::Sender<AlertDraft>, cancel: CancellationToken) -> anyhow::Result<()>;
}
```
Manager restarts a crashed watcher with backoff (5 s, 30 s, 5 min) and raises `info`
`watcher-failed` after 3 crashes. Matchers are pure functions `fn(&JournalEntry) ->
Option<AlertDraft>` so they are unit-testable with JSON fixtures.

Journal source: one `journalctl -f -o json --since now` child; parse `MESSAGE`,
`_SYSTEMD_UNIT`, `SYSLOG_IDENTIFIER`, `PRIORITY`, `_PID`, `_COMM`, `__REALTIME_TIMESTAMP`.
Kernel lines have `_TRANSPORT=kernel`.

## 3. System rules (S13)

| rule | Source / match | Threshold | Severity | Cooldown | dedupeKey |
|---|---|---|---|---|---|
| `oom-kill` | kernel: `Out of memory: Killed process (\d+) \((.+?)\)` or `oom-kill:` | each | review | 1 h | `oom:<comm>` |
| `kernel-warning` | kernel: `^(WARNING:|BUG:|kernel BUG|general protection fault|Oops)` | each | review | 6 h | `kwarn:<first 60 chars>` |
| `segfault` | kernel: `(\S+)\[(\d+)\]: segfault at` | 3 in 10 min same comm | info | 6 h | `segv:<comm>` |
| `unit-failed` | poll `systemctl --failed --output=json` 60 s (+ `--user`) | new failed unit | review | 6 h | `unit:<scope>:<unit>` |
| `disk-space` | statvfs on mounts from `/proc/mounts` (ext4, btrfs, xfs, vfat >1 GiB) every 5 min | >= 90% review, >= 97% high | review/high | 12 h | `disk:<mount>` |
| `thermal` | `sensors -j` every 30 s: any temp >= its `high` (or 90 C if absent) for 2 consecutive min; GPU `nvidia-smi` temp >= 85 C | sustained | review | 1 h | `thermal:<sensor>` |
| `throttling` | `nvidia-smi` `clocks_throttle_reasons.active` thermal bits, or kernel `cpu clock throttled` | sustained 2 min | info | 6 h | `throttle:<device>` |
| `dkms-broken` | on daemon start and when `uname -r` changed since last run: `dkms status` lines not `installed` for the running kernel, or NVIDIA module not loaded while the card is present | each | high | 24 h | `dkms:<kernel>` |
| `smart` | daily priv `smartctl` per disk: health not PASSED, media errors > 0, reallocated > 0, percentage used >= 90 | change vs last | high | 24 h | `smart:<dev>:<attr>` |
| `security-updates` | daily `apt list --upgradable` lines containing `-security` | count > 0 | info | 24 h | `secupd` |
| `apparmor-denied` | kernel/audit: `apparmor="DENIED"` | 5 in 10 min same profile | info | 6 h | `aa:<profile>` |
| `ssh-bruteforce` | unit ssh/sshd: `Failed password for (invalid user )?(\S+) from (\S+)` or `Invalid user (\S+) from (\S+)` | >= 10 per source IP in 5 min | review (high if the IP is external and sshd listens on a non-LAN address) | 1 h | `ssh-bf:<ip>` |
| `ssh-login` | `Accepted (password|publickey) for (\S+) from (\S+)` | each, when webcam/idle says owner away or between quiet hours | review | none | `ssh-in:<ip>:<user>` |
| `sudo-new` | `/etc/group` sudo members changed (inotify) | each | high | none | `sudoers-group` |

## 4. Intrusion detection on the PC (S14)

| rule | Mechanism | Severity | Evidence |
|---|---|---|---|
| `honeytoken-access` | inotify `IN_OPEN | IN_ACCESS` on registered honeytokens; ignore events while KAgent itself reads them (internal flag) | high | path, time; with S14b bpftrace stream: pid, ppid, comm, exe (`/proc/<pid>/exe`), cmdline, user, process tree (walk ppid) |
| `honeytoken-modified` | `IN_MODIFY | IN_DELETE_SELF | IN_MOVE_SELF | IN_ATTRIB` | high | same + new hash |
| `persist-authorized-keys` | baseline diff of `~/.ssh/authorized_keys` (and `/root/.ssh/authorized_keys` via `read_root` if allowed) | high | added key fingerprints (`ssh-keygen -lf`) |
| `persist-unit` | new unit/timer file in `~/.config/systemd/user/`, `/etc/systemd/system/`, `/etc/systemd/user/`; inotify + 10 min diff | review (high if ExecStart points into `/tmp`, `/dev/shm`, `~/.cache`, or a hidden dir) | unit file content (first 40 lines) |
| `persist-cron` | crontab hash change (`crontab -l`), `/etc/cron.*` new files | review | diff |
| `persist-suid` | hourly (not in the first round after a start, unless there is no baseline yet) `nice -n 19 ionice -c 3 find /usr /opt /home -xdev -user root -perm -4000 -type f` vs baseline | high for new file | path, owner, mtime, sha256, `dpkg -S` owner package (none = suspicious) |
| `persist-preload` | `/etc/ld.so.preload` exists or changed; `LD_PRELOAD` in `/proc/*/environ` of user processes (hourly sample) | high | content |
| `module-new` | `/proc/modules` set vs baseline + kernel log `module verification failed` / taint bits O, E newly set | review (high if unsigned and not from dkms) | name, `modinfo` filename/signer |
| `sudoers-change` | `/etc/sudoers.d/*`, `/etc/pam.d/*` hash change (`read_root` allowed for sudoers.d) | high | file, diff when readable |
| `exec-suspicious` (S16, opt-in) | exec-snoop stream: shell (`sh, bash, dash, zsh, python* -c, perl -e, nc, ncat, socat`) whose parent comm is a service daemon or browser (`firefox, chrome, chromium, thunderbird, java, node` under a service, `apache2, nginx, php-fpm`), or exe path under `/tmp`, `/dev/shm`, `/run/user/*/` | high | process tree |
| `reverse-shell` (S16, opt-in) | every 30 s for interactive shells: fd 0/1 of the process is a socket (`readlink /proc/<pid>/fd/0` = `socket:[...]`) with a remote peer in `ss -tpn` | high | pid, peer address, tree |
| `listener-new` | `ss -tulpnH` every 5 min vs set seen in the last 7 days | review | port, process, exe |
| `agent-silent` | user timer `kagent-heartbeat.timer` (every 2 min) checks `<data>/heartbeat` mtime < 5 min, else `notify-send -u critical "KAgent stopped"` | high | n/a (handled outside the daemon) |

Baseline rules: first run takes the baseline silently (`info` "baseline taken").
Changes made by KAgent's own change journal are added to the baseline automatically
(matched by path + time window of the change), everything else alerts.

Honeytokens (opt-in, created by `POST /api/sentinel/honeytokens`):
- `~/.aws/credentials.legacy`: `[backup]` profile, fake `AKIA` id with the marker in
  the secret.
- `~/.ssh/id_backup` + `.pub`: a real ed25519 keypair generated for this purpose, never
  added to any authorized_keys.
- `~/Documents/passwords-old.kdbx`: 64 KiB random bytes with a KeePass header magic.
- `~/.config/backup.env`: fake `OPENAI_API_KEY=sk-...marker`, `DB_PASSWORD=...`.
- Print the instruction to exclude these from Baloo indexing
  (`balooctl config add excludeFolders` / file list) so the indexer does not trigger them;
  also ignore events from `baloo_file` if pid attribution is active.

## 5. Network and router rules (S15)

| rule | Source | Threshold | Severity | Cooldown | dedupeKey |
|---|---|---|---|---|---|
| `lan-new-device` | DHCP leases + ARP every 2 min; MAC not in `devices` | each | review | none | `newdev:<mac>` |
| `router-scanner` | address list `scanners` new entries (poll 2 min) | each | info (review if source is inside the LAN) | 24 h | `scan:<ip>` |
| `router-login-failed` | `/log` topics `system,error,critical` `login failure for user (\S+) from (\S+)` | >= 3 in 10 min | review | 1 h | `rtr-login:<ip>` |
| `router-trap-account` | login (success or failure) of the decoy router user `admin2` (created by owner per setup) | each | high | none | `rtr-trap:<ip>` |
| `flow-new-destination` | a LAN device talks to a destination /24 never seen for that device, between 01:00-06:00 local, > 1 MiB | each | info | 24 h | `newdst:<src>:<dst/24>` |
| `flow-volume-spike` | device bytes in a 5 min bucket > 10x its 7-day median for that hour and > 200 MiB | each | review | 6 h | `spike:<src>` |
| `flow-port-fanout` | one LAN source to >= 50 distinct dst ports or >= 30 distinct LAN hosts in 5 min | each | high | 1 h | `fanout:<src>` |
| `flow-honeypot-touch` | any flow to the OpenCanary host IP (settings) from the LAN | each | high | 1 h | `canary:<src>` |
| `router-unreachable` | REST fails 5 times in a row (10 min) | each | info | 6 h | `rtr-down` |

Containment is never automatic. Proposals reference `router_block_ip` (external IPs)
or "move device to quarantine VLAN" (owner action, documented, not automated in S15).

## 6. Tests

- Each matcher: 1 positive + 1 negative fixture (journal JSON lines, `sensors -j`
  output, `ss` output, REST JSON, IPFIX bytes).
- Dedupe/cooldown with a fake clock.
- Baseline diff: added/removed/changed items, own-change suppression.
