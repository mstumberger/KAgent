# Spec: notices as toasts (part of S43)

Milestone: `docs/milestones/v0.0.2.md` section 0, item 6. Built with S43
(`work-windows.md`), after S42 turned the header into a title bar.

Why: a provider error ("429 Too Many Requests") is one yellow line in the header today
(`App.tsx`: `<span className="notice">`). It cannot be closed, the next one replaces it,
and the title bar of S42 has no room for it.

## 1. The event

`AgentEvent::Notice` grows from `{level, text}` to:

```json
{"type": "notice", "level": "info|warn|error", "text": "...",
 "key": "provider:gemini:429", "sessionId": "...", "kind": "error|recovery|info"}
```

- `key`: what makes two notices the same one. Provider errors: `provider:<id>:<class>`
  with class `quota`, `key`, `busy`, `down`, `other` (the classes of `fallback_notice`,
  `compact/loop_side.rs`). Hooks: `hook:<name>`. A notice without a key is never merged.
- `kind` defaults by level: `error` and `warn` are `error`, `info` is `info`. The loop
  sends `recovery` with the same `key` when a provider that failed answers again
  ("Gemini answers again").
- `action`: `{label, path, body}`, optional: a button of the toast. Pressing it posts `body` to `path` of
  the daemon's API (a path under `/api/`); the toast closes when the answer is a success, and a refusal
  is shown as a notice of its own (`<label> failed: <message>`) while the toast stays. The reader's human
  check uses it: label "Open it", path `/api/web/reader/open`, body `{url}`, key `reader:check:<host>`
  (`web.md` section 9).
- `sessionId` when the notice belongs to a turn. An old client ignores the new fields.
- The page's own failures (`store.ts` `set({notice})`: "Todo list not saved") become
  local notices with `kind: "error"` and no key.

## 2. The stack (`web/src/components/Toasts.tsx`, `toasts.css`)

- Place: top right of the window, under the title bar, 12 px from the edge, above the
  content and under dialogs. In the widget layout (340 px): full width minus 8 px each
  side, at the top.
- A toast: an icon by level, the text (at most 3 lines, the rest behind "more"), a
  count chip "x3" when the same `key` came again, the time of the latest, and a close
  button (`aria-label` "Dismiss"). With `sessionId` that is not the open session: a
  link "Open conversation".
- Same `key` while its toast is open: the count goes up, the text and time are
  replaced, the toast moves to the top. No second toast.
- Lifetime: `error` stays until closed. `recovery` closes the open error toast of its
  `key` and shows itself for 6 s. `info` stays 6 s. Hover or focus holds the timer.
- At most 4 are shown; older ones fold into "2 more" (a click opens them). "Dismiss
  all" appears with 2 or more.
- Closed toasts are not stored; a reload starts empty. The last 20 notices of the
  session are in the work window's Activity tab (`work-windows.md`).
- Accessibility: the stack is `role="region"` `aria-label="Notifications"`; an error
  toast is `role="alert"`, the others `role="status"`; Escape closes the focused one;
  the close buttons are in the tab order; `prefers-reduced-motion` drops the slide.
- The header's `.notice` span and its rule go. Voice: an error toast is also said once
  per `key` in a live session ("Gemini is out of quota"), by the existing status line.

## 3. Tests

- Core: the classes of a provider error and their keys; a recovery after an error has
  the same key; none without a previous error.
- Web, headless (dark and light, 1200x820 and 340x460): two errors of one key are one
  toast "x2"; a recovery replaces it and goes after 6 s; an error stays; close; "Dismiss
  all"; the fifth folds; Escape; the header has no notice line.
