# Spec: screen watch (S48)

Milestone: `docs/milestones/v0.0.2.md` section 0, item 9. Terms: `GLOSSARY.md` (Screen
watch; not a Watcher). Depends on: S46 (`vision.md`: capture rules), S47
(`local-models.md`: a local vision model), `voice.md` section 12, `notices.md`.

The owner asks KAgent to keep an eye on the screen for a goal and a time: "tell me when
the build finishes", "watch for errors for the next 20 minutes". KAgent looks at a
frame every few seconds with a local vision model and speaks up when the goal is met.
It never watches without being asked, and no frame is stored.

## 1. One watch

```json
{"id": "w_1a2b", "sessionId": "...", "goal": "tell me when the build in the terminal finishes",
 "target": "window|monitor", "windowId": "0x0460000a", "windowTitle": "konsole: cargo build",
 "until": "2026-11-28T10:40:00+01:00", "everySec": 10, "mode": "once|repeat",
 "state": "watching|met|expired|stopped|failed", "looks": 42, "lastLookAt": "...",
 "lastNote": "still compiling (312/480)"}
```

- At most one watch at a time (a second `start` is refused with the running one's goal).
- `until`: required, at most 2 hours from the start; default 20 minutes.
- `everySec`: 5 to 60, default 10.
- `mode`: `once` ends at the first hit (the default); `repeat` reports every new hit
  until the time is up (for "watch for errors"), at most one report per 30 s.
- `target`: the window that is active when the watch starts (default, followed by its
  id even when it is no longer active; if it closes, the watch ends as `failed: the
  window was closed`), or the monitor.
- State is in memory only: a daemon restart ends every watch (told to the owner in the
  next session start: "the screen watch ended with the restart").

## 2. Each look (`screen_watch/look.rs`)

1. Capture the target (the rules of `vision.md` section 3: never KAgent's own cards,
   never the lock screen; a locked session pauses the watch and does not count looks).
2. Cheap skip: the frame is scaled to 64x36 grey and compared with the last looked-at
   frame; under 1.5 % changed pixels and no look skipped more than 6 times in a row,
   nothing is sent to a model.
3. The local vision model (the model of the `visionModel` setting when its provider is
   local, else the recommended installed local vision model; none: section 4) gets the
   frame (longest side 1024) and: "Goal: <goal>. Answer JSON only: {\"met\": true|false,
   \"note\": \"at most 12 words on what you see about the goal\", \"confidence\": 0..1}".
   A reply that is not that JSON counts as `met: false` with the note "unreadable
   answer"; 5 in a row end the watch as `failed`.
4. `met` with confidence under 0.6: the next look comes after 2 s and both must agree.
5. Optional cloud check (`screenWatch.cloudConfirm`, default off): a hit is confirmed
   by the cloud vision model with the same frame before it is reported. That one frame
   leaves the machine; nothing else ever does.
6. The frame is dropped. Frames are never written to disk and never put in a session;
   only `lastNote` is kept.

## 3. A hit

- The session gets a message from KAgent (role assistant, marked `via: "watch"`):
  "Screen watch: <note>. (goal: <goal>)". With a live voice session open it is spoken
  as a `[SAY]` turn; else a desktop notification through the notifier (and Telegram
  when `screenWatch.telegram` is on).
- `once`: the watch ends as `met`. `repeat`: it goes on.
- Expiry without a hit: one line, "Screen watch ended after 20 minutes: <lastNote>",
  by the same channels.
- The owner's next message after a hit is a normal turn; the model has the watch
  message in the history and may look itself with `screen`.

## 4. Tool, settings, API

Tool `screen_watch` (group `desktop`):

| Action | Assess | Input | What |
|---|---|---|---|
| `start` | MUT, `always_ask`, rememberable for the session | `{goal: string (<= 200), minutes?: 1..120, everySec?: 5..60, mode?: once\|repeat, target?: window\|monitor}` | Starts the watch. Summary: `watch <window title or "the monitor"> for <n> minutes: <goal>`. From voice it is asked out loud like every `always_ask` call (`voice-control.md` section 4). |
| `status` | RO | `{}` | The watch, or "no screen watch". |
| `stop` | RO-equivalent | `{}` | Ends it as `stopped`. "Stop watching" by voice is also caught by the daemon's stop phrases when a watch runs. |

- No local vision model: `start` is refused with "Screen watch needs a local vision
  model: Settings -> Local models recommends <id> for this machine."
- Settings section `screenWatch`: `{enabled: true, cloudConfirm: false, telegram: false,
  defaultMinutes: 20, everySec: 10}` on the Desktop and apps page (a card "Screen
  watch"), with the model it uses named there. `cloudConfirm` is a protected key of the
  `settings` tool (it lets frames leave the machine).
- API: `GET /api/screen-watch` (the watch or null), `DELETE /api/screen-watch` (stop).
  Event `screenWatchChanged {watch}` on each state change and at most every 5 s with
  `looks` and `lastNote`.
- UI: while a watch runs, a chip in the title bar next to the voice chip: an eye icon,
  the minutes left, the goal as its title; a click opens a small popover (goal, target,
  last note, looks, Stop). The widget shows the eye and the minutes. The chip is always
  visible while frames are being taken, in every view: the owner must never be watched
  without seeing it.
- The kill switch (`POST /stop-all`) ends the watch. Quiet hours do not.

## 5. Load

One look of a 4B vision model on the owner's card takes about a second. With the skip
of step 2 an idle screen costs almost nothing. The daemon measures each look; when the
median of the last 5 is over half of `everySec`, it doubles the interval (up to 60 s)
and says so in `lastNote`. A watch never runs while a voice turn is thinking on the
same local model (it waits for the turn).

## 6. Files

```
crates/kagentd/src/screen_watch/mod.rs    the one watch, its loop, states
crates/kagentd/src/screen_watch/look.rs   capture, the skip, the prompt, the JSON reply
crates/kagentd/src/screen_watch/tool.rs
crates/kagentd/src/screen_watch/http.rs
web/src/components/WatchChip.tsx
web/src/settings/sections/Desktop.tsx     the card
```

## 7. Tests (no network, no real screen: a fake capture and a scripted vision model)

- The loop: a hit on the third look ends a `once` watch with one message; `repeat`
  reports twice and not more often than every 30 s; expiry says the last note; stop;
  the kill switch; a second `start` is refused; a closed window fails the watch.
- `look`: the skip under 1.5 % and its limit of 6; the JSON reply and each malformed
  one; low confidence needs two looks; `cloudConfirm` sends exactly one frame to the
  cloud fake and none without it; no frame file exists after a run (the data folder is
  listed before and after).
- A locked session pauses; the interval doubles when looks are slow.
- Tool: the summary; refused without a local vision model; asked from voice in `full`.
- Web, headless: the chip appears, counts down, its popover stops the watch.
- With the owner: "tell me when this build finishes" on a real `cargo build`, by voice.
