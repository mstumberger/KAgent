# Spec: recommended local models (S47)

Milestone: `docs/milestones/v0.0.2.md` section 0, item 8. Terms: `GLOSSARY.md`
(Recommended model). Depends on: `models.md` (providers, roles, Fetch models),
`platform.md` (hardware probes).

A Settings page "Local models": what this machine can run, what KAgent recommends for
each job, and a Download button. The models run in Ollama; KAgent installs nothing but
models, and only when the owner clicks or confirms.

## 1. The machine (`GET /api/local-models`)

```json
{"ollama": {"reachable": true, "version": "0.9.1", "baseUrl": "http://127.0.0.1:11434"},
 "gpu": {"name": "NVIDIA GeForce RTX 3060", "vramTotalMb": 12288, "vramFreeMb": 10366,
         "source": "nvidia-smi"},
 "ramTotalMb": 64000,
 "installed": [{"id": "qwen2.5:7b-instruct", "sizeMb": 4683, "jobs": ["chat"]}],
 "recommended": [ ...section 2, each with "fit" and "installed"... ],
 "pulls": [{"id": "gemma3:4b", "state": "pulling", "doneMb": 812, "totalMb": 3300}]}
```

- GPU through `kagent_core::platform`: `nvidia-smi --query-gpu=name,memory.total,
  memory.used --format=csv,noheader,nounits`; AMD from
  `/sys/class/drm/card*/device/mem_info_vram_total`; none: `gpu: null` and the fit is
  judged against RAM (CPU inference, marked "slow"). Several GPUs: the largest.
- `installed` from Ollama `GET /api/tags`. Ollama not reachable: the page shows how to
  start it and nothing else; KAgent does not install Ollama.

## 2. The list (`config/local-models.json`, shipped, read-only)

Hand-kept, updated with releases; `{"schemaVersion": 1, "models": [...]}`:

```json
{"id": "gemma3:4b", "name": "Gemma 3 4B", "jobs": ["vision", "chat"],
 "sizeMb": 3300, "vramMb": 4500, "context": 8192, "tools": false,
 "languages": ["en", "sl"], "note": "Reads screenshots; light enough to sit next to the desktop.",
 "engine": "ollama"}
```

- `jobs`: `chat`, `vision`, `embedding`, `stt`. `vramMb`: what it needs loaded at its
  default quantisation with `context`. `tools`: whether it calls tools reliably (a chat
  model without it is not offered as the Brain, only as a helper).
- First content (sizes are checked against Ollama's library when the stage is built,
  and a model that does not exist there is left out, never guessed):
  - chat: `qwen2.5:7b-instruct` (tools), `qwen2.5:14b-instruct` (tools), `llama3.1:8b`
    (tools), `gemma3:12b` (no tools; Slovenian).
  - vision: `gemma3:4b`, `gemma3:12b`, `qwen2.5vl:7b`, `llama3.2-vision:11b`.
  - embedding: `nomic-embed-text`, `bge-m3` (multilingual).
  - stt: Whisper `large-v3-turbo` and `small` (`engine: "whisper"`): listed, marked
    "KAgent cannot use a local speech model yet (planned)", no Download in v0.0.2.
- Fit, per entry, from the GPU of section 1 (pure function, tested):
  - `fits`: `vramMb` <= 85 % of `vramTotalMb`.
  - `tight`: up to 100 % ("fits when nothing else uses the card").
  - `too big`: above; with `gpu: null`, `slow` when `vramMb` <= 50 % of RAM, else
    `too big`.
  - A second line when `vramMb` > `vramFreeMb` now: "<n> MB are in use by other
    programs at the moment".
- One entry per job is marked "Recommended": the largest that `fits`, preferring
  `tools` for chat and the owner's language (`persona.language`) when it is listed.

## 3. Downloading

- `POST /api/local-models/pull {id}`: only an id of the shipped list (anything else is
  400: this is not a general model installer). Starts Ollama `POST /api/pull` (stream)
  and answers 202; progress goes out as `localModelsChanged {pulls}` (at most twice a
  second) and is in `GET /api/local-models`.
- Before it starts: free disk at Ollama's model folder must be `sizeMb` + 2 GB (else
  409 with the numbers); at most one pull at a time, others queue.
- `DELETE /api/local-models/pull/:id` cancels (Ollama keeps the partial layers; a later
  pull resumes). A failed pull keeps its error on the row with Retry.
- Done: the daemon runs Fetch models on the Ollama provider (`models.md` section 3) so
  the model appears there, switched on, with the roles of its `jobs` (`vision` and
  `chat` map to role `chat` with `supportsImage` for vision; `embedding` has no role).
  It is not made the Brain or anything else by itself; the row offers "Use for
  vision" / "Use as fallback" / "Use as Brain" (a `PUT /api/settings`, the role check of
  `models.md` applies).
- `DELETE /api/local-models/:id` removes an installed model of the list (Ollama `DELETE
  /api/delete`), refused with 409 while a setting names it.
- No Ollama provider yet: the first pull adds it from its template (`models.md`).

## 4. The page (Settings -> Local models; `ui-settings.md`: under Basics, after Models)

1. Machine card: the GPU and a VRAM bar (used by others, free), RAM, Ollama's state and
   version. No GPU: "No graphics card found: models run on the processor and are slow."
2. One group per job (Chat, Vision, Embedding, Speech to text), each a list of rows:
   name, the id in mono, size, the fit chip (`fits` green, `tight` amber, `too big`
   grey, `slow` amber), the note, and one of: Download (disabled with the reason when
   `too big`), a progress bar with Cancel, or "Installed" with its "Use for ..." button
   and Remove. The recommended row carries the "Recommended" badge and sits first.
3. Installed models that are not on the list: a plain list at the bottom, no buttons
   ("managed in Settings -> Models").

## 5. For the agent

Tool `local_models` (group `agent`, RO): `{}` gives the machine and the list with fit,
so "which local model should I use for vision?" has an answer. It does not pull: the
answer names the page, or the model asks the owner and the owner clicks. (A pull by
voice is not in v0.0.2.)

## 6. Files

```
config/local-models.json
crates/kagent-core/src/platform/gpu.rs       the probes, parsers with real samples
crates/kagentd/src/local_models/mod.rs       state, fit, recommend (pure parts tested)
crates/kagentd/src/local_models/pull.rs      the Ollama stream, queue, cancel, disk check
crates/kagentd/src/local_models/http.rs      the routes
crates/kagentd/src/local_models/tool.rs
web/src/settings/sections/LocalModels.tsx, web/src/settings/local-models/*
```

## 7. Tests (no network: a fake Ollama on loopback)

- `gpu`: the parsers on recorded `nvidia-smi` output (one card, two cards, the "N/A"
  of a card without memory reporting) and the sysfs files; no GPU.
- Fit and recommend: the table of section 2 for 12 GB, 8 GB, 4 GB and no GPU; the
  language preference; a chat model without tools is never the recommended Brain.
- The shipped file parses; every id is unique; `deny_unknown_fields`.
- Pull: an id outside the list is 400; progress events; the disk check; one at a time;
  cancel; a failed stream; Fetch models after success gives the roles; remove is 409
  while a setting names the model.
- Web, headless with a mocked API: each fit chip, a running pull, Cancel, "Use for
  vision", the no-GPU and the no-Ollama states, dark and light.
- With the owner: download `gemma3:4b` (the model of screen watch) and set it for vision.
