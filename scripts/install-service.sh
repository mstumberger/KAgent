#!/usr/bin/env bash
# Owner-run (user level, no sudo): start kagentd with your graphical session, restart
# it if it crashes. Logs: journalctl --user -u kagentd -f
# Remove: systemctl --user disable --now kagentd && rm ~/.config/systemd/user/kagentd.service
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
bin="$repo/target/release/kagentd"
[ -x "$bin" ] || { echo "build first: cargo build --release -p kagentd"; exit 1; }
dir="$HOME/.config/systemd/user"; mkdir -p "$dir"
cat > "$dir/kagentd.service" <<UNIT
[Unit]
Description=KAgent system agent daemon
After=graphical-session.target pipewire.service
PartOf=graphical-session.target

[Service]
WorkingDirectory=$repo
ExecStart=$bin serve
Restart=on-failure
RestartSec=3
# The desktop session environment (DISPLAY, DBUS) is imported by systemd --user.

[Install]
WantedBy=graphical-session.target
UNIT
systemctl --user daemon-reload
systemctl --user enable --now kagentd.service
echo "kagentd runs as a user service. Open: http://127.0.0.1:7878/#token=\$(cat ~/.config/kagent/token)"
