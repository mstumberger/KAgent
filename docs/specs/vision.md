# Spec: images in chat and "look at my screen" (S46)

Milestone: `docs/milestones/v0.0.2.md` section 0, item 7. Depends on: `tools.md`
section 15 (`screen`), `models.md` (the `visionModel` setting, `supportsImage`),
`voice.md` section 12.

Today a message is text only (`protocol.rs` `Message.content: String`), and the `screen`
tool sends a screenshot to the vision model and returns words. The owner cannot give
KAgent a picture, and in `live` voice the model that talks never sees the screen itself.

## 1. Images in a message

- `Message` gets `images: Vec<ImageRef>` (empty is left out of the file):
  `{id, mime: "image/png|jpeg|webp", width, height, bytes}`. The pixels are a file,
  `<data>/sessions/<sessionId>.images/<id>.<ext>`, not in the session's JSONL.
- Limits: 4 images per message, 8 MB each as uploaded. The daemon re-encodes: longest
  side at most 1568 px, PNG kept for screenshots (few colours), else JPEG 85; metadata
  (EXIF, location) is dropped. Other types are refused with 415.
- API: `POST /api/sessions/:id/images` (multipart, one file) answers the `ImageRef`;
  `POST /api/sessions/:id/messages` takes `images: [id]`; `GET
  /api/sessions/:id/images/:imageId` serves it (bearer token; `Cache-Control: private`).
  Deleting a session deletes its folder.
- Composer (`web/src/components/CommandInput.tsx`): paste (Ctrl+V of an image), drop on
  the conversation, and a paperclip button. Thumbnails (56 px) above the input with a
  remove button each; sending with only images is allowed. A message shows its images
  at up to 320 px, a click opens the full one.

## 2. Who reads them

For a turn whose message has images:

1. The Brain reads them itself when its resolved config has `supportsImage` (the
   adapters send image parts: `openai_chat` `image_url` data URLs, `anthropic` `image`
   blocks, `ollama` `images`).
2. Else the daemon asks `visionModel` once per image ("Describe this image for another
   model: every text in it word for word, the layout, what is selected or wrong.") and
   puts the answer in place of the image: `[image 1, read by <model id>: ...]`. The
   description is stored with the message so it is made once.
3. Else (no vision model): the turn is refused before it starts, with the notice "The
   Brain cannot read images: pick a vision model in Settings -> Models."

- Context: an image costs tokens. Compaction (`compaction.md`) replaces the images of
  summarised messages by their description (made then, if there is none) and keeps the
  files. Only the images of the last 6 messages are sent as pixels; older ones go as
  their description.
- Privacy: an image goes where the conversation goes (the Brain's provider, or the
  vision model's). The composer says so once per session when that provider is not
  local: "Images are sent to <provider>."
- Subagents, tasks and Telegram: a Telegram photo sent to the bot becomes an image of
  that message (`bots.md` forward path, same limits). Tasks and subagents carry none.

## 3. "Look at my screen" in voice

- `live` and `auto`: the live model gets `screen` in its tool set whatever
  `voice.toolGroups` holds. For a live call the tool does not ask the vision model: it
  captures (active window by default; `target: "monitor"|"desktop"` when asked), and
  sends the image to the live session itself as a `realtimeInput` video frame (JPEG,
  longest side 1024), then answers the call with `{"status": "ok", "output": "the
  screenshot is in your view now"}`. Gemini Live then answers from what it sees.
- A frame leaves the machine, so the call needs approval as `screen` does today
  (rememberable for the session). From voice: asked out loud once per session ("I want
  to look at your active window. Say yes to go ahead, or no."); the yes covers later
  looks in the same voice session.
- `routed`: unchanged; the Brain calls `screen` and its answer is read aloud.
- The instruction of `live` and `auto` gets: "To see the screen call `screen`; never
  describe the screen without having looked in this turn."
- Never captured: KAgent's own approval cards and the lock screen (a locked session
  answers "the screen is locked"); the capture of the active window leaves other
  windows out.

## 4. Files

```
crates/kagent-core/src/protocol.rs          Message.images, ImageRef
crates/kagent-core/src/session/images.rs    store, re-encode, delete with the session
crates/kagent-core/src/agent/vision.rs      who reads: Brain, vision model, or refuse
crates/kagent-providers/src/adapters/*.rs   image parts per API format
crates/kagentd/src/api/images.rs            the three routes
crates/kagentd/src/screen.rs                the live path: frame to the session
crates/kagentd/src/live/mod.rs              realtimeInput video frame; screen declared
web/src/components/CommandInput.tsx, web/src/components/Attachments.tsx, MessageImages.tsx
```

## 5. Tests

- Core: an image round-trips through the session file and survives a restart; limits
  and the 415; re-encode size and no EXIF; delete with the session; the three readers
  (a Brain with `supportsImage` gets pixels, one without gets the description once, no
  vision model refuses); only the last 6 messages carry pixels.
- Adapters: the request body of each API format with one image (recorded fixtures).
- Live (`scripts/test-live.py`): a `screen` call sends one video frame and an ok
  response; the first asks, the second in the session does not.
- Web, headless: paste, drop, remove, send, the thumbnail and the full view; the
  provider line once.
- With the owner: paste a screenshot of an error and ask what it says; by voice, "look
  at my screen, what is this dialog?".

## 6. As built (2026-11-28)

The decisions are in `IMPLEMENTATION.md` section E, session 2026-11-28 (S46). What differs from the
text above: `ImageRef` also has `description` and `readBy`; the file folder holds an `<id>.json` for
each image; the re-encode keeps PNG up to 8192 distinct colours; the live `screen` has its own spec
(`target` only); a look is refused while any approval card waits, and while the screen is locked.

