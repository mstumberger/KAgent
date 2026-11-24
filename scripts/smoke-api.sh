#!/usr/bin/env bash
# S4 check: start the daemon with a temp home, add the local Ollama provider,
# run one tool-using question end to end. Requires: curl, jq, ollama with
# qwen2.5:7b-instruct. Exits non-zero on any failure.
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH"
export KAGENT_HOME=$(mktemp -d /tmp/kagent-smoke.XXXXXX)
export KAGENT_NO_TRAY=1
PORT=7878
BASE="http://127.0.0.1:$PORT"
MODEL="qwen2.5:7b-instruct"

cleanup() { [[ -n "${DAEMON_PID:-}" ]] && kill "$DAEMON_PID" 2>/dev/null || true; }
trap cleanup EXIT

echo "== building kagentd"
cargo build -q -p kagentd

echo "== starting daemon (KAGENT_HOME=$KAGENT_HOME)"
./target/debug/kagentd serve >"$KAGENT_HOME/daemon.log" 2>&1 &
DAEMON_PID=$!

for i in $(seq 1 50); do
  TOKEN_LINE=$(grep -o "token=[0-9a-f]*" "$KAGENT_HOME/daemon.log" | head -1 || true)
  [[ -n "$TOKEN_LINE" ]] && break
  sleep 0.2
done
TOKEN=${TOKEN_LINE#token=}
[[ -n "$TOKEN" ]] || { echo "daemon did not start:"; cat "$KAGENT_HOME/daemon.log"; exit 1; }
AUTH="Authorization: Bearer $TOKEN"

api() { curl -sS -H "Host: 127.0.0.1:$PORT" -H "$AUTH" "$@"; }

echo "== host check (expect 421)"
CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "Host: evil.example" "$BASE/api/state" -H "$AUTH")
[[ "$CODE" == "421" ]] || { echo "host check failed: $CODE"; exit 1; }

echo "== auth check (expect 401)"
CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "Host: 127.0.0.1:$PORT" "$BASE/api/state")
[[ "$CODE" == "401" ]] || { echo "auth check failed: $CODE"; exit 1; }

echo "== adding ollama provider"
api -X POST "$BASE/api/providers" -H 'Content-Type: application/json' \
  -d '{"templateId":"ollama"}' | jq -e '.id == "ollama"' >/dev/null

echo "== fetching models from ollama"
api -X POST "$BASE/api/providers/ollama/fetch-models" | jq -e ".models | length > 0" >/dev/null

echo "== setting chat model"
api -X PUT "$BASE/api/settings" -H 'Content-Type: application/json' \
  -d "{\"chatModel\":\"ollama::$MODEL\"}" | jq -e ".chatModel == \"ollama::$MODEL\"" >/dev/null

echo "== creating session"
SID=$(api -X POST "$BASE/api/sessions" -H 'Content-Type: application/json' -d '{}' | jq -r .id)

echo "== asking: what kernel am I running? (up to 180 s)"
UNAME_R=$(uname -r)
api -X POST "$BASE/api/sessions/$SID/messages" -H 'Content-Type: application/json' \
  -d '{"text":"What kernel version am I running? Use your tools."}' | jq -e '(.status == "started") or (.status == "queued")' >/dev/null

TOOL_SEEN=0
ANSWER=""
for i in $(seq 1 360); do
  sleep 0.5
  DONE=$(api "$BASE/api/sessions/$SID" | jq -r '[.messages[] | select(.role == "tool") | .toolName] | join(",")')
  if [[ "$DONE" == *"system_info"* || "$DONE" == *"kernel_info"* ]]; then TOOL_SEEN=1; fi
  RUNNING=$(api "$BASE/api/state" | jq -r --arg sid "$SID" '.running | index($sid) != null')
  if [[ "$RUNNING" == "false" ]]; then break; fi
done
ANSWER=$(api "$BASE/api/sessions/$SID" | jq -r '[.messages[] | select(.role == "assistant") | .content] | last // ""')

echo "tool calls seen: $DONE"
echo "final answer: ${ANSWER:0:300}"

[[ "$TOOL_SEEN" == "1" ]] || { echo "FAIL: no system_info/kernel_info tool call"; exit 1; }
[[ "$ANSWER" == *"$UNAME_R"* ]] || { echo "FAIL: answer does not contain $UNAME_R"; exit 1; }
echo "SMOKE OK"
