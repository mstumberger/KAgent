#!/usr/bin/env bash
# Owner-run (user level, no sudo): a systemd user timer that warns with a critical
# desktop notification when kagentd stops writing its heartbeat (crash, killed by an
# intruder, ...). Remove: systemctl --user disable --now kagent-heartbeat.timer
set -euo pipefail
data="${KAGENT_HOME:+$KAGENT_HOME/data}"; data="${data:-$HOME/.local/share/kagent}"
dir="$HOME/.config/systemd/user"; mkdir -p "$dir"
cat > "$dir/kagent-heartbeat.service" <<UNIT
[Unit]
Description=KAgent heartbeat check

[Service]
Type=oneshot
ExecStart=/bin/sh -c 'f="$data/heartbeat"; if [ ! -f "\$f" ] || [ \$(( \$(date +%%s) - \$(stat -c %%Y "\$f") )) -gt 300 ]; then notify-send -a KAgent -u critical "KAgent stopped" "No heartbeat for 5 minutes. If you did not stop it, check what happened."; fi'
UNIT
cat > "$dir/kagent-heartbeat.timer" <<UNIT
[Unit]
Description=KAgent heartbeat check every 2 minutes

[Timer]
OnBootSec=5min
OnUnitActiveSec=2min

[Install]
WantedBy=timers.target
UNIT
systemctl --user daemon-reload
systemctl --user enable --now kagent-heartbeat.timer
echo "Heartbeat timer installed (checks $data/heartbeat every 2 minutes)."
