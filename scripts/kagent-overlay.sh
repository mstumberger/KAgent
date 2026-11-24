#!/usr/bin/env bash
# Open the KAgent presence overlay in a chrome-less browser window (no Tauri needed).
# Starts kagentd if it is not running. Usage: kagent-overlay.sh [--talk]
set -euo pipefail
cfg="${KAGENT_HOME:+$KAGENT_HOME/config}"; cfg="${cfg:-$HOME/.config/kagent}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
if ! (exec 3<>/dev/tcp/127.0.0.1/7878) 2>/dev/null; then
  bin="$(command -v kagentd || echo "$repo/target/release/kagentd")"
  [ -x "$bin" ] || bin="$repo/target/debug/kagentd"
  nohup "$bin" serve >/dev/null 2>&1 &
  for _ in $(seq 40); do (exec 3<>/dev/tcp/127.0.0.1/7878) 2>/dev/null && break; sleep 0.2; done
fi
token="$(cat "$cfg/token")"
q="overlay"; [ "${1:-}" = "--talk" ] && q="overlay&autostart"
url="http://127.0.0.1:7878/?$q#token=$token"
browser="$(command -v brave-browser || command -v brave || command -v google-chrome || command -v chromium || command -v chromium-browser || command -v microsoft-edge || true)"
if [ -n "$browser" ]; then
  exec "$browser" --app="$url" --window-size=380,560 --class=kagent-overlay
fi
# No Chromium-family browser (e.g. stock Fedora): a normal Firefox window works too.
exec firefox --new-window "$url"
