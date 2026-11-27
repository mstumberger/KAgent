# Spec: Telegram bot (S29)

Talk to KAgent from a phone, and receive alerts, approvals and automation results
there. The daemon keeps listening on 127.0.0.1 only: the bot uses long polling over an
outbound HTTPS connection to api.telegram.org, so no port is opened and the Host check
stays as it is.

## 1. Settings and secrets (config.md section 11)

- `telegram`, off by default.
- Bot token in Secret Service (`app=kagent, kind=telegram, id=bot`); file fallback
  `<config>/telegram.secret` (0600) when `KAGENT_HOME` is set, as for provider keys.
  The daemon keeps the token in memory only.
- Pairing file `<config>/telegram.json` (0600). One chat is paired.
- The UI says next to the toggle that Telegram's servers see every message exchanged.
- The poll runs while a token is set and `enabled` is on, paired or not (the pairing
  message must arrive); a change of the settings wakes it. Its state (`off`,
  `disabled`, `unpaired`, `polling`, `error`) is reported by `GET /api/telegram` and by
  the `telegramStatus` event. An error stays until a poll succeeds (a retry every 5 s).

## 2. Pairing

1. The owner creates a bot with @BotFather and pastes the token (Settings -> Telegram).
   `POST /api/telegram/token` calls `getMe` and stores the token only when Telegram
   knows it (400 for a wrong shape or a refusal, 502 `telegram_unreachable` without an
   answer), and the bot's username is shown.
2. The owner clicks "Show pairing code": six digits, valid 10 minutes, one use (400
   with no token or while switched off, 409 when a chat is paired).
3. The owner sends `/start <code>` to the bot. The first correct code pairs that chat.

- An update from any other chat is dropped and logged, before anything else happens.
- `/start` from the paired chat answers with the status.
- Unpair (UI, or deleting the token) forgets the chat; deleting the token leaves
  `enabled` as it is.
- Wrong codes (decision of 2026-11-27): after 5 wrong codes the waiting code is replaced
  by a new one and pairing is locked for 10 minutes. While locked, no code is checked,
  not even the right one, and the chat is told how many minutes are left. The new code
  (shown in Settings > Telegram) is good for 10 minutes after the lock ends; a code asked
  for during the lock is too. `GET /api/telegram` reports `pairingLockedUntil` while it
  lasts. The count is in memory and starts again after a lock or a pairing.

## 3. Messages (`kagentd/src/bots/telegram*.rs`, `bots/telegram_api.rs`)

- Long polling: `getUpdates` with `timeout: 30` and `allowed_updates:
  ["message", "callback_query"]`; the HTTP client's timeout is 40 s. The offset is kept
  in memory (and sent only once there is one).
- Commands: `/start <code>`, `/new` (a new session), `/status` (doctor score, open
  alerts, running turns), `/stop` (stop all), `/help`.
- Free text: a turn in the paired chat's session (origin `telegram`, `Via::Telegram`),
  reused until `/new`.
- Reply: "Working on it..." is sent before the turn starts; the last assistant message
  replaces it (`editMessageText`), split at 4096 chars on paragraph boundaries. Markdown
  becomes Telegram HTML (`<`, `>` and `&` escaped; bold, italic and code kept; headings
  and tables become plain text). Directive lines such as `::widget{...}` are dropped.
- While a turn runs, further messages queue (the agent takes them as steering).
- At most 3 turns a minute per chat. A message is cut at 4000 chars.
- Voice notes: when a Gemini chat model is enabled, the file is downloaded from
  api.telegram.org (a plain relative path only) and sent to `generateContent` as
  `inlineData` (`audio/ogg`) with "Transcribe this voice message in its language." The
  transcript is handled as text. Without such a model the bot answers that voice notes
  need a Gemini model.
- Photos and other attachments: "I cannot read attachments here yet."

## 4. Approvals from the phone

- With `forwardApprovals` on, a card pending for `approvalForwardSec` (default 20) is
  sent to the paired chat with two buttons, Allow once and Deny. The message shows the
  tool, the summary and the risk. The rule is applied again after the delay, so a card
  answered on the desktop meanwhile is not forwarded.
- Never forwarded: destructive calls and plan cards. Root actions only with
  `allowPrivileged`. "Always" is never offered from the phone.
- A button press resolves the card (`broker.resolve`) and the message is edited to show
  the outcome, without its buttons. A press on an already resolved card (one answered
  on the desktop keeps its buttons on the phone) answers "Already resolved" and edits
  nothing.

## 5. Alerts and automations

- New alerts of severity `review` and `high` are pushed when `notifyAlerts` is on:
  `review` respects `watchers.quietHours`, `high` always goes. The message has an Ack
  button.
- Automations with `telegram` in `deliver` get their result (`automations.md` section
  3): the `Telegram` channel of the notifier (foundations.md section 4) gets its sink
  here, and reaches the paired chat.

## 6. API and events

api.md section 6. `telegramStatus` is emitted on every change of the polling state
(`off`, `disabled`, `unpaired`, `polling`, `error`). `PUT /api/telegram` saves as `PUT
/api/settings` does and sends `settingsChanged`.

## 7. UI: Settings -> Telegram (group "Channels")

- Status card: off; not configured; waiting for pairing (with the bot username);
  connected (chat id, since when); error.
- Token field (paste, Save), "Show pairing code" (the code and its expiry), Unpair.
- Toggles: push alerts, forward approvals and its delay, allow root actions.
- "Send test message".
- A line about remote control: "Whoever can write in this Telegram chat can ask KAgent
  to do things. Approvals follow the permission mode."
- The page handles `telegramStatus` itself and reads the status again. No Telegram icon
  exists in `Icon.tsx`: the menu item uses `talk`.

## 8. Security

- Only the paired chat is served.
- Telegram turns follow the permission mode: in `trusted` and `full` they run without
  cards, like every other turn. The UI says so.
- Destructive actions never come from Telegram: their cards are not forwarded.
- The token never leaves the daemon; the API reports only `configured`, and no error
  text holds the token (it is checked before it goes into a URL path).
- Rate limit: 20 API calls a minute; `retry_after` is honoured.
- Requests go to `https://api.telegram.org` only (in tests: a loopback fake whose base
  URL is injected). A file path returned by Telegram is accepted only when it is a
  plain relative path (no scheme, no `..`).

## 9. Tests (no network; a fake `TelegramApi` trait and an injected clock)

- Update parsing fixtures: text; a command with an argument; a callback query; a voice
  note; an unsupported update.
- Pairing: a wrong code; an expired code; a used code; a second chat refused; `/start`
  in the paired chat answers the status.
- Splitting: the 4096 limit; splits on paragraphs; never inside a code block.
- Markdown to Telegram HTML: escaping and the supported tags.
- Forwarding rules table: destructive never; privileged per setting; plan never.
- Rate limiter with a fake clock; the file path guard.
- The real client against a loopback fake of the Bot API: `getMe`, one long poll,
  `sendMessage` with the keyboard, `editMessageText` with an empty keyboard,
  `answerCallbackQuery`, `retry_after`, the limit of 20 calls a minute, a refused call,
  no token in any error, no other address.
- The whole flow with fakes, through the polling task and the work of the routes: a
  message of the paired chat starts a turn; a second chat is dropped before anything; a
  card forwarded after its delay and resolved by Allow once; destructive and plan cards
  never; a card answered on the desktop is not forwarded; alerts with their rules and
  Ack; the `Telegram` channel of the notifier; `telegramStatus` on every change;
  deleting the token unpairs.
