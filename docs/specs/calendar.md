# Spec: Google Calendar and the morning report (S45)

Milestone: `docs/milestones/v0.0.2.md` section 0, item 3. Terms: `GLOSSARY.md`
(Morning report). Depends on: S44 (`voice-control.md` section 4: calls from voice ask),
`voice.md` section 12, `bots.md` (Telegram delivery), `web.md` (the search fallback).

A native module of the daemon, like the Telegram bot: no plugin, no third-party server.
KAgent talks to Google's APIs directly with the owner's own OAuth client.

## 1. Connection (OAuth, the owner's own client)

- The owner creates a Google Cloud project, enables the Calendar API, and creates an
  OAuth client of type "Desktop app". The page takes its client id and client secret.
- Flow: authorization code with PKCE and a loopback redirect to the daemon:
  `http://127.0.0.1:<port>/api/calendar/oauth/callback`. `POST /api/calendar/connect`
  answers `{url}` (the page opens it in the default browser, `xdg-open`); the callback
  checks `state` (32 random bytes, single use, 10 minutes), exchanges the code and shows
  a small "Connected, you can close this tab" page. The callback is the one calendar
  route without the bearer token; it accepts loopback only and a known `state` only.
- Scopes: `https://www.googleapis.com/auth/calendar.events` and
  `.../auth/calendar.calendarlist.readonly`. Nothing else is asked for.
- Storage: the client secret and the refresh token in the desktop keyring
  (`app=kagent, kind=calendar`), with the 0600 file `<config>/calendar.json` as the
  fallback (the `TokenStore` of bots and web search, generalised over kind and file).
  The file and the keyring entry are on the token guard's list (`token-guard.md`).
  The access token lives in memory only.
- Owner note, shown on the page: an OAuth consent screen left in "Testing" makes
  Google expire the refresh token after 7 days. Publish it ("In production"; no
  verification is needed for your own account) or reconnect weekly.
- A refresh that fails with `invalid_grant` sets the state `reconnect` and raises one
  notice; nothing is deleted.
- Disconnect: revokes the token at Google, deletes it locally, keeps the settings.

## 2. Settings (`calendar` section; the page is Settings -> Calendar)

```json
"calendar": {
  "enabled": false,
  "clientId": "",
  "calendars": ["primary"],
  "defaultCalendar": "primary",
  "defaultDurationMin": 60,
  "home": "",
  "travelMarginMin": 10,
  "report": {
    "enabled": true,
    "weekdays": "07:45",
    "weekends": "09:00",
    "spoken": true,
    "telegram": true,
    "tomorrow": true,
    "tasks": true,
    "leaveBy": true
  }
}
```

- `calendars`: the ids the agent and the report may see; every other calendar of the
  account does not exist for them (enforced in the client, not only in the page).
  `defaultCalendar` must be one of them and writable.
- `home`: a free address; resolved once to coordinates when it is saved with a Maps key
  (stored as `homeLatLng`); without a key it is kept as text and `leaveBy` is off.
- The Maps API key is not in settings: keyring, `kind=maps`. `weekdays`/`weekends` are
  `HH:MM` or `""` (no report on those days).
- Page (ui-settings.md pattern, one section file plus `settings/calendar/`):
  1. Connection: the two client fields, Connect / Reconnect / Disconnect, the account's
     address, the state (`off`, `connecting`, `connected`, `reconnect`, `error`), the
     Testing note, a "How to create the client" fold with the five steps.
  2. Calendars: the account's calendars, a switch each, the default as a radio; a
     read-only calendar cannot be the default.
  3. Places: the Maps key field (masked), Test, the home address, the margin.
  4. Morning report: on/off, the two times, the channel switches (Telegram is disabled
     with "pair the bot first" when no chat is paired), the three content switches, and
     "Send now" (delivers to the switched-on channels and shows the text).

## 3. Tools (group `calendar`, new; default on once connected)

| Tool | Assess | Input | What |
|---|---|---|---|
| `calendar_events` | RO, network | `{from?: date or datetime, to?: date or datetime, query?: string, calendar?: id, max?: 1..50}` | The events of the switched-on calendars in the range (default: today), sorted by start: id, calendar, title, start, end, all-day, location, Meet link, guests' names, and `overlaps` with the ids it collides with. Times in the machine's time zone, with the weekday. |
| `calendar_change` | MUT, network, `always_ask`; `delete` is destructive | `{action: "add"\|"update"\|"delete", id?: string, calendar?: id, title?, start?, end?, durationMin?, allDay?, location?: {name, address, mapsUrl?}, notes?, reminderMin?: number\|null, meet?: bool, guests?: [email]}` | `add` needs title and start; `end` or `durationMin` (default `defaultDurationMin`). `update` and `delete` need `id`. Guests get Google's invitation mail (`sendUpdates=all`) only when `guests` is given. `meet: true` asks Google for a Meet link. Returns the stored event and its link. |
| `place_search` | RO, network | `{query: string, near?: string}` | Up to 3 candidates: name, address, `mapsUrl`, and `source`: `places` or `web`. |

- Summary of a change (the card, and what the voice says before it, `voice.md` section
  12): `add "Lunch with Miha", Fri 28 Nov 12:00 to 13:00, at Thai Orchid, Glavni trg 5,
  Maribor, calendar Personal, reminder 30 min` / `move "<title>" from <old> to <new>` /
  `delete "<title>", <when>`. The summary is built from the stored event for `update`
  and `delete` (one read before the question), never from the model's words.
- Permission: `add` and `update` are `always_ask` (a card in `ask` and `trusted`, none
  in chat under `full`, and always asked when the call comes from voice). `delete` is
  destructive: asked in every mode, and by voice only "confirm" answers.
- Refusals: a calendar that is not switched on; a read-only calendar; `end` before
  `start`; a start more than 5 years away; more than 20 guests; an `id` that is not in a
  switched-on calendar. Recurring events: `update` and `delete` act on the one instance
  named by `id`; the whole series is refused with "change the series in Google
  Calendar" (v0.0.2).
- Undo: a change is recorded in the change journal (`config.md`); undo of `add` deletes
  the event, of `delete` restores it from the stored copy, of `update` writes the old
  fields back.

## 4. The interview (adding an event)

The model's job, guided by the description of `calendar_change` and by the shipped
skill `skills/calendar-event/SKILL.md` (loaded on "add an event", "schedule", "dodaj
dogodek"). One question at a time, in the owner's language:

1. Title, day and time (skip what the owner already said). A relative day ("Friday")
   is said back as a date.
2. How long (offer the default, 1 hour).
3. Where. With an answer: `place_search`, read the best candidate back ("Thai Orchid,
   Glavni trg 5, Maribor: that one?"); with several plausible ones, name up to three.
   "No place" is an answer.
4. Calendar (only when more than one is switched on; offer the default) and reminder
   (offer Google's default).
5. Notes; a Meet link only when the owner asked for a call. Guests only when the owner
   mentioned people; an email the model does not have is asked for, never guessed.
6. `calendar_change add`. From voice the daemon's question reads the whole event back
   and waits for the spoken yes; in chat the card shows it.

The owner can say "just add it" at any point: the model fills the defaults and calls.

## 5. Places and travel (`calendar/places.rs`)

- With a Maps key: Places API (New) `places:searchText` (field mask: display name,
  formatted address, location, `googleMapsUri`), biased to `near` or to `home`. Travel:
  Routes API `computeRoutes`, `DRIVE`, `TRAFFIC_AWARE`, departure at the event's start
  minus the no-traffic duration.
- Without a key, or when the Places call fails: `web_search` for the query, the address
  taken from the first result that has one, and `mapsUrl` =
  `https://www.google.com/maps/search/?api=1&query=<name, address>`; `source: "web"`,
  and the read-back says "I could not check this on the map".
- The event gets `location` = "name, address" and the Maps link as the first line of
  the description.
- Hosts are fixed (`oauth2.googleapis.com`, `www.googleapis.com`,
  `places.googleapis.com`, `routes.googleapis.com`); no redirect is followed. Keys and
  tokens never appear in a tool result, a log line or an error.

## 6. Morning report (`calendar/report.rs`)

- A timer inside the module (not a task of `tasks.json`: the owner cannot delete it by
  accident, and it needs no model). It fires at `weekdays` on Monday to Friday and at
  `weekends` on Saturday and Sunday, once per day; a daemon started after the time
  delivers the report once if it is before 12:00.
- The text is built by code, in `persona.language` (sl or en; "auto" follows the last
  language the owner used):
  1. "Good morning, <address>. Today, <weekday date>:" then each event: time (or "all
     day"), title, place. Overlaps: "<A> and <B> overlap from 10:00 to 10:30." No
     events: "Nothing in your calendar today."
  2. Leave-by (with `leaveBy`, a Maps key, `home` and a first event with a place):
     "Leave by 08:05 for <title>: 25 minutes by car with traffic, plus 10."
  3. Tomorrow (with `tomorrow`): the first event of tomorrow, when it starts before
     10:00 or is the only one.
  4. Tasks (with `tasks`): KAgent's own reminders and scheduled tasks due today.
- Channels:
  - `telegram`: sent at the report time through the bot's notifier (`bots.md`).
  - `spoken`: the report waits. At the first live voice session that connects after
    the report time and before midnight, the daemon sends it as a `[SAY]` text turn
    after the greeting (`voice.md` section 2; the `SAY` sentence is added to the `live`
    instruction too). It is spoken once; "repeat the morning report" asks the model to
    call `calendar_report`.
  - The page shows the last report's text and when and where it went.
- Tool `calendar_report` (RO, group `calendar`): `{day?: "today"|"tomorrow"}`, the
  report's text for the model ("what's on today?").
- Not connected or `reconnect`: no report; one notice the first time, not every day.

## 7. API

| Route | Body | Answer |
|---|---|---|
| GET `/api/calendar` | | `{state, account?, calendars: [{id, name, color, writable, on}], hasClientSecret, hasMapsKey, lastReport?: {at, text, sent: [..]}, error?}` |
| PUT `/api/calendar/client` | `{clientId, clientSecret? /* "" keeps */}` | 204 |
| POST `/api/calendar/connect` | | `{url}`; 400 without a client |
| GET `/api/calendar/oauth/callback` | `?code&state` (loopback, no bearer) | a small HTML page |
| POST `/api/calendar/disconnect` | | 204 |
| PUT `/api/calendar/maps-key` | `{key /* null clears */}` | 204 |
| POST `/api/calendar/maps-test` | | `{ok, place?, error?}` (one search for "Ljubljana") |
| POST `/api/calendar/report` | `{deliver: bool}` | `{text, sent: [..]}` |

The `calendar` settings section goes through `PUT /api/settings`. Event `calendarChanged
{state}` when the connection state changes.

## 8. Files

```
crates/kagentd/src/calendar/mod.rs       the hub: types, state, construction
crates/kagentd/src/calendar/connection.rs   the client, sign-in and callback, token refresh, disconnect, the settings it saves
crates/kagentd/src/calendar/oauth.rs     PKCE, state, code exchange, refresh, revoke
crates/kagentd/src/calendar/google.rs    calendarList, events list/insert/patch/delete; the switched-on rule
crates/kagentd/src/calendar/model.rs     calendars and events as read from Google, and their text (pure)
crates/kagentd/src/calendar/places.rs    Places, Routes, the web fallback
crates/kagentd/src/calendar/place_tool.rs   place_search
crates/kagentd/src/calendar/report.rs    when it is due, the language, the text (pure)
crates/kagentd/src/calendar/report_run.rs   the timer, the text for a day, delivery, the spoken one
crates/kagentd/src/calendar/report_tool.rs  calendar_report
crates/kagentd/src/calendar/tools.rs     what the tools share, and calendar_events
crates/kagentd/src/calendar/plan.rs      what calendar_change is asked: checks, Google's JSON, the sentences (pure)
crates/kagentd/src/calendar/change.rs    calendar_change, the journal records, the undo handler
crates/kagentd/src/calendar/state.rs     <data>/calendar-state.json
crates/kagentd/src/calendar/http.rs      the routes
crates/kagentd/src/calendar/settings.rs  the section, its checks
crates/kagentd/src/calendar/fake.rs, fake_api.rs, fake_maps.rs   a fake Google for the tests (as bots/fakes.rs)
skills/calendar-event/SKILL.md           the interview
web/src/settings/sections/Calendar.tsx, web/src/settings/calendar/*
```

## 9. Tests (no network: every Google host is the fake)

- `oauth`: the URL (scopes, PKCE challenge, loopback redirect); a callback with an
  unknown, used or old `state` is refused; the exchange stores the token; a refresh on
  401; `invalid_grant` gives `reconnect`; revoke on disconnect.
- `google`: only switched-on calendars are read; paging; the time zone and all-day
  events; overlaps; the insert body (location, description with the Maps link,
  reminder, Meet request, guests with `sendUpdates`); a single instance of a series.
- `tools`: each refusal; the three summaries, built from the stored event; `delete` is
  destructive; `add` from voice asks in `full`; the journal record and each undo.
- `places`: the field mask; the bias; no key and a failed call fall back to the web
  with `source: "web"`; a key never appears in an error.
- `report`: the text for a day with events, an overlap, no events, an all-day event,
  in sl and en; leave-by arithmetic; the timer's days and times, once per day, the late
  start; spoken once at the first session; Telegram off when not paired.
- Web, headless with a mocked API: each card of the page in each state; Connect opens
  the URL; the read-only calendar cannot be the default.
- Scenarios (`scenarios.md` S45), with the owner: connect; "what's on today?"; "let's
  add an event" by voice through the interview with a place; move it; delete it with
  "confirm"; "Send now"; the report spoken the next morning.

## 10. Owner actions (IMPLEMENTATION.md section G)

1. console.cloud.google.com: a project; enable "Google Calendar API"; OAuth consent
   screen (External, your address as a test user, then "Publish app"); Credentials ->
   OAuth client ID -> Desktop app; paste id and secret into Settings -> Calendar.
2. For places and leave-by: enable "Places API (New)" and "Routes API", create an API
   key restricted to those two, paste it into the page. Billing must be on for the
   project; this use stays far inside the free monthly allowance.
