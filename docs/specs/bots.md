# Spec: Telegram bot (S29)

Talk to KAgent from a phone, and receive alerts, approvals and automation results
there. The daemon keeps listening on 127.0.0.1 only: the bot uses long polling over an
outbound HTTPS connection to api.telegram.org, so no port is opened and the Host check
stays as it is.

## 1. Settings and secrets (config.md section 11)

- `telegram`, off by default.
- Bot token in Secret Service (`app=kagent, kind=telegram, id=bot`); file fallback
  `<config>/telegram.secret` (0600) when `KAGENT_HOME` is set, as for provider keys.
- Pairing file `<config>/telegram.json` (0600). One chat is paired.
- The UI says next to the toggle that Telegram's servers see every message exchanged.

## 2. Pairing

1. The owner creates a bot with @BotFather and pastes the token (Settings -> Telegram).
   `POST /api/telegram/token` stores it and calls `getMe` to show the bot's username.
2. The owner clicks "Show pairing code": six digits, valid 10 minutes, one use.
3. The owner sends `/start <code>` to the bot. The first correct code pairs that chat.

- An update from any other chat is dropped and logged, before anything else happens.
- `/start` from the paired chat answers with the status.
- Unpair (UI, or deleting the token) forgets the chat.

## 3. Messages (`kagentd/src/bots/telegram.rs`, `bots/telegram_api.rs`)

- Long polling: `getUpdates` with `timeout: 30` and `allowed_updates:
  ["message", "callback_query"]`; the HTTP client's timeout is 40 s. The offset is kept
  in memory.
- Commands: `/start <code>`, `/new` (a new session), `/status` (doctor score, open
  alerts, running turns), `/stop` (stop all), `/help`.
- Free text: a turn in the paired chat's session (origin `telegram`, `Via::Telegram`),
  reused until `/new`.
- Reply: "Working on it..." at once; the final text replaces it (`editMessageText`),
  split at 4096 chars on paragraph boundaries. Markdown becomes Telegram HTML
  (`<`, `>` and `&` escaped; bold, italic and code kept; headings and tables become
  plain text). Directive lines such as `::widget{...}` are dropped.
- While a turn runs, further messages queue (the agent takes them as steering).
- At most 3 turns a minute per chat. A message is cut at 4000 chars.
- Voice notes: when a Gemini chat model is enabled, the file is downloaded from
  api.telegram.org and sent to `generateContent` as `inlineData` (`audio/ogg`) with
  "Transcribe this voice message in its language." The transcript is handled as text.
  Without such a model the bot answers that voice notes need a Gemini model.
- Photos and other attachments: "I cannot read attachments here yet."

## 4. Approvals from the phone

- With `forwardApprovals` on, a card pending for `approvalForwardSec` (default 20) is
  sent to the paired chat with two buttons, Allow once and Deny. The message shows the
  tool, the summary and the risk.
- Never forwarded: destructive calls and plan cards. Root actions only with
  `allowPrivileged`. "Always" is never offered from the phone.
- A button press resolves the card (`broker.resolve`) and the message is edited to show
  the outcome. A press on an already resolved card answers "Already resolved".

## 5. Alerts and automations

- New alerts of severity `review` and `high` are pushed when `notifyAlerts` is on:
  `review` respects `watchers.quietHours`, `high` always goes. The message has an Ack
  button.
- Automations with `telegram` in `deliver` get their result (`automations.md` section
  3): the `Telegram` channel of the notifier (foundations.md section 4) gets its sink
  here.

## 6. API and events

api.md section 6. `telegramStatus` is emitted on every change of the polling state
(`off`, `disabled`, `unpaired`, `polling`, `error`).

## 7. UI: Settings -> Telegram

- Status card: off; not configured; waiting for pairing (with the bot username);
  connected (chat id, since when); error.
- Token field (paste, Save), "Show pairing code" (the code and its expiry), Unpair.
- Toggles: push alerts, forward approvals and its delay, allow root actions.
- "Send test message".
- A line about remote control: "Whoever can write in this Telegram chat can ask KAgent
  to do things. Approvals follow the permission mode."

## 8. Security

- Only the paired chat is served.
- Telegram turns follow the permission mode: in `trusted` and `full` they run without
  cards, like every other turn. The UI says so.
- Destructive actions never come from Telegram: their cards are not forwarded.
- The token never leaves the daemon; the API reports only `configured`.
- Rate limit: 20 API calls a minute; `retry_after` is honoured.
- Requests go to `https://api.telegram.org` only. A file path returned by Telegram is
  accepted only when it is a plain relative path (no scheme, no `..`).

## 9. Tests (no network; a fake `TelegramApi` trait)

- Update parsing fixtures: text; a command with an argument; a callback query; a voice
  note; an unsupported update.
- Pairing: a wrong code; an expired code (fake clock); a used code; a second chat
  refused; `/start` in the paired chat answers the status.
- Splitting: the 4096 limit; splits on paragraphs; never inside a code block.
- Markdown to Telegram HTML: escaping and the supported tags.
- Forwarding rules table: destructive never; privileged per setting; plan never.
- Rate limiter with a fake clock.
- File path guard.
