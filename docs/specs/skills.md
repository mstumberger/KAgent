# Spec: skills library, self-authoring and sources (S18, S20, S21)

KAgent reads `SKILL.md` skills in the format Claude Code, Codex and ZCode use, so the
existing ecosystem installs as is. Three additions over the S7 loader: a merged
library with several roots instead of one directory, the agent writing its own skills,
and importing skills from other tools or git repositories.

Not in scope: plugins (bundles of skills, hooks and MCP servers), a hosted store UI,
project-scoped skills.

## 1. Skill format

- A directory containing `SKILL.md`. Bundled and user skills are `<root>/<name>/`;
  repositories are scanned two levels deep (`<dir>/SKILL.md` and
  `<dir>/<name>/SKILL.md`) because repositories group skills in category folders.
- Frontmatter (YAML between `---` lines). Supported: `key: value`, quoted scalars,
  block scalars `>` `>-` `|` `|-` with indented continuation lines, and nested
  mappings (kept as raw text, not interpreted).
  - `name`: optional. Default and fallback: the directory name. Must match
    `^[a-z0-9][a-z0-9-]{0,63}$` else the skill is skipped with a warning.
  - `description`: recommended, at most 1024 chars after unfolding. Missing = empty.
  - `when_to_use`: optional string.
  - `license`, `metadata`: ignored.
  - A line that is not `key: value` is ignored. It never invalidates the skill.
- A skill without frontmatter is valid: its name is the directory name and its
  description is the first non-empty line of the body (cut at 140 chars).
- The body follows the frontmatter. At most 100 KB, else the skill is skipped.
- One level of comments (`#`) inside the frontmatter is ignored.

## 2. Roots and precedence

| Source | Root | Notes |
|---|---|---|
| `bundled` | repo `skills/`, or next to the binary, or `/usr/share/kagent/skills` (`paths.rs` existing candidates) | read-only; the 8 playbooks and `skill-creator` |
| `user` | `<config>/skills/<name>/` | written by the owner in the UI or by the agent (`skill_save`) |
| `imported` | `<config>/skills-imported/<name>/` | written by import and sources; a directory, or a symlink into another tool's skills folder |

- All roots are merged on every name. On a name clash the higher row wins (user >
  imported > bundled); the others are `shadowed` and are not loaded.
- Within a root, symlinks are followed for reading. The same real path reached twice
  counts once.
- `skills.disabled` (settings) hides skills by name, from every root. Bundled skills
  can be disabled, never deleted.

## 3. Library and reload (`kagent-core/src/skills/`)

- `skills.rs` (157 lines) becomes a module directory:
  - `frontmatter.rs`: `parse(raw) -> Option<SkillDoc { name, description, when_to_use, body }>` and `unfold_scalar`.
  - `library.rs`: `SkillSource`, `SkillEntry`, `SkillLibrary`.
  - `tool.rs`: `LoadSkill`.
- `SkillEntry { name, description, when_to_use, source, path (file), dir, enabled, shadowed, size_bytes, body }`.
- `SkillLibrary::scan(roots, disabled) -> SkillLibrary`. A fingerprint per root (the
  directory mtime and entry count) lets `refresh_if_changed()` answer "unchanged"
  with a few `stat` calls.
- The daemon builds the prompt through its config closure once per turn
  (`main.rs`, `ConfigSource`). From S18 on that closure calls
  `library.refresh_if_changed()` first, so a new or edited skill is in the next
  prompt without a restart. A change emits `skillsChanged`.
- `LoadSkill` holds the library, not a directory.

## 4. Prompt index and usage

- Index lines, enabled skills only: `- name: description` under the heading
  `Skills (call load_skill with the name before using one)`.
- `skills.indexBudgetChars` (default 3000): lines are added in order user, imported,
  bundled, each alphabetical, until the budget is spent. The remaining names follow on
  one line: `Also available (names only): a, b, c`.
- The tool list line of the prompt ("Available tools: ...") is unchanged.
- `load_skill` counts a use in `<data>/skill-usage.json` (`uses`, `lastUsed`).

## 5. Writing skills

- Agent tool: `skill_save` (tools.md section 17). Approval follows the permission mode
  like every other change: a card in `ask`, voice-answerable, none in `trusted` and
  `full`. The card preview shows the text, so the owner sees what will steer the agent
  later.
- Owner: Settings -> Skills, "New skill" (editor) or "New skill with the agent" (opens
  chat with `/skill-creator` prefilled).
- Bundled `skills/skill-creator/SKILL.md` (ported from ZCode's skill-creator guidance,
  60 lines at most): what a skill is for; write the description as trigger words in
  third person; keep `when_to_use` for the cases the description cannot hold; steps
  name the tools; end with "a good answer looks like"; under 60 lines; when to offer a
  skill (the owner asks to remember how something was done; a non-trivial fix is likely
  to recur; the same diagnosis was done twice). Never write secrets or machine-specific
  absolute paths into a skill; say "the router" and let memory hold the address.
- Prompt rule (working rules): "After a non-trivial fix that is likely to recur,
  offer to save it as a skill. Save only what the owner asks for or agrees to."

## 6. Slash commands (chat composer)

- Typing `/` opens a picker of enabled skills and the built-in commands
  (`/compact`, `/plan`, `/new`). `/<skill-name> <request>` sends `/<skill-name>
  <request>` as the user message.
- The daemon (`api::post_message`) sees a message that starts with `/<name>` where
  `name` is an enabled skill: the user message is stored as sent, and the turn gets
  the skill body as an extra prompt block for every model request of that turn. The
  body is not stored in the session. How: `prompt::skill_block` renders the block and
  `api::post_message` hands it to `Agent::send_with` as `TurnOpts.system_suffix`
  (`PromptInput` cannot carry it: the config closure has no input of a single turn).
- The message arrives while a turn runs (it is queued as steering): its block is in
  the prompt from the next model request on.
- Unknown `/name`, or a disabled one: sent as plain text (the model answers normally).
- A skill loaded this way counts as a use.
- `/new` (the whole message) is done by the UI: a new session, nothing is sent.
  `/compact` and `/plan` are listed as coming with S22 and S24 until those stages.

## 7. Import from other tools (S21)

- Detected sources (`GET /api/skills/import-sources`): every folder of the table is a
  row, with `exists` false and no skills when it is missing:

| agent | path |
|---|---|
| ZCode | `~/.zcode/skills` |
| shared (`.agents`) | `~/.agents/skills` |
| Claude Code | `~/.claude/skills` |
| Codex | `~/.codex/skills` |
| OpenCode | `~/.config/opencode/skills`, `~/.opencode/skills` |
| Goose | `~/.config/goose/skills` |
| Kiro | `~/.kiro/skills` |
| Windsurf | `~/.codeium/windsurf/skills` |

- The list shows every skill found with name, description, real path, whether it is
  already installed (same real path or same name) and which name it duplicates.
  Duplicates of one real path appear once.
- Import mode `copy` (default) copies the directory tree (files only, at most 20 MB per
  skill, no symlinks inside) into `<config>/skills-imported/<name>`. Mode `symlink`
  creates `<config>/skills-imported/<name>` as a symlink to the source directory.
- Import never overwrites: a name that any skill has (a bundled one too), or that has a
  folder or link in the imported root, is skipped with the reason `name exists`.
- **Quarantine**: imported skills arrive disabled (added to `skills.disabled`). The
  owner enables the ones they want in Settings -> Skills. The import result says so.
- Delete on an imported symlink removes the link only. The target is never touched.

## 8. Sources: git URLs (S21)

- A source = `{id, name, url, ref, subdir, commit?, lastSync?, lastError?, skills[]}` in
  `<config>/skill-sources.json` (`kagentd/src/skills_sources.rs`).
- `url`: `https://` git URL, or an absolute local path. `ref`: branch, tag or commit
  (default: the default branch). `subdir`: folder inside the repository that holds the
  skills (default: the repository root). `name`: display name (default: repository
  name).
- Adding a source (`POST /api/skills/sources`): clone with
  `git -c core.hooksPath=/dev/null -c protocol.file.allow=user clone --depth 1
  [--branch <ref>] -- <url> <data>/skill-sources/<id>`, scan `subdir` two levels deep,
  copy each skill into `<config>/skills-imported/<name>` (same copy rule as above),
  record `path`, `hash` (SHA-256 of all files, sorted) and `syncedAt`. A name that any
  skill has, or that has a folder in the imported root, is skipped with `name exists`.
  The same address cannot be added twice (409). A source is read without following
  links.
  - A local path is read in place, not cloned.
  - Limits: clone output is discarded; the directory may hold at most 200 MB and 2000
    files; more than 100 skills is an error and nothing is installed; `git` missing ->
    error "install git".
  - Nothing in the source is executed. Only `SKILL.md` and the files beside it are
    copied.
- `sync` re-fetches (`git fetch --depth 1 origin <ref>` (`HEAD` when no `ref` is
  given), then `git checkout --force --detach FETCH_HEAD`, or re-reads the local path),
  copies new and changed skills, removes copies whose skill disappeared from the
  source, and reports `added`, `updated`, `removed`. New and changed skills are
  disabled again (changed content is new content).
- Deleting a source removes its clone; with `removeSkills` it also removes the copies
  it installed.
- Catalog of suggestions (`config/skill-sources.builtin.json`, read-only):

| name | url | subdir | note |
|---|---|---|---|
| Anthropic skills | `https://github.com/anthropics/skills` | `skills` | document and example skills, Apache 2.0 |
| Superpowers | `https://github.com/obra/superpowers` | `skills` | workflow skills, MIT |
| OpenAI skills | `https://github.com/openai/skills` | `skills/.curated` | deprecated upstream, kept for its content |

## 9. UI: Settings -> Skills (S20, S21)

Layout per `ui-settings.md`: list on the left, detail on the right.

- Toolbar: search, filter (All, On, Off, Bundled, Mine, Imported), and the actions
  Import, Add source, Rescan, New skill.
- List rows: toggle, name, description (one line), source badge
  (`bundled` / `mine` / `imported`), `updated N days ago`, `used N times`. An imported
  skill that is off carries an `off` badge.
- Detail: name, source, path (click = copy), description, `when_to_use`, usage, the
  body as Markdown with an Edit switch (editor: name (new only), description,
  `when_to_use`, body, Save, Cancel), and the actions Enable/Disable, Duplicate (to a
  user skill), Delete (confirm), Open folder, and for imported skills the source.
- Import dialog: source tree (agent, path, skills), select all / none, mode, result
  counts and skip reasons, and the line "Imported skills are off until you enable
  them".
- Sources dialog: table of sources (name, url, skills, last sync, error), Sync, Remove;
  add form (URL or path, ref, subdir) with the catalog as one-click suggestions.
- Empty state when there are no user skills: two buttons, New skill, Import.

## 10. Security

- Skill text is data for the model, like tool output. The prompt keeps the rule.
- Imported and sourced skills are disabled until the owner enables them.
- `skill_save` and `skill_delete` go through the permission engine and the audit log.
  `skill_delete` refuses everything outside `<config>/skills` and refuses symlinks.
  `skill_save` writes no skill through a symlink either.
- After `skill_save`, `skill_delete` and every write of the API the library is read
  again at once and `skillsChanged` goes out: `load_skill` and the Skills page see the
  change before the next turn.
- Sources are added by the owner through the UI or the API only. No tool adds a source.
- A source clone runs with hooks disabled and is never executed.

## 11. Tests (unit; no network)

- `frontmatter.rs`: fixtures under `crates/kagent-core/tests/fixtures/skills/` are the
  real frontmatter blocks of at least 12 installed skills, including the three the
  current loader drops (`discernment-nudge`, `claude-api`, `academy-guide`), folded and
  literal block scalars, quoted scalars, a `metadata` mapping, comments, no frontmatter,
  a name that is not a valid id (skipped).
- `library.rs`: precedence user > imported > bundled; `disabled` hides a bundled skill;
  the same real path twice counts once; an oversized body is skipped; the index respects
  `indexBudgetChars` and lists the rest as names; `refresh_if_changed` reports a change
  after a file is added, removed or edited.
- `tool.rs`: `load_skill` returns the body with the directory variable expanded, counts
  a use, and names the enabled skills grouped by source on an unknown name.
- `author.rs` (`author/tests.rs`): `SkillSave` refuses a reserved name, a name equal to
  another one except for case, and an oversized body, and overwrites only a user skill.
  `SkillDelete` refuses bundled and imported names, refuses a symlink, and removes a
  user directory. `file.rs`: `render` writes what `parse` reads back, and `replace`
  changes three fields of an existing file and keeps the other lines of its
  frontmatter. The bundled `skill-creator` is listed, loads and is under 60 lines.
- `sources.rs`: parse of `skill-sources.json` round trip; argv built for clone, fetch
  and checkout; invalid URL and `ref` are rejected before any command runs; the sync
  plan (added, updated, removed) from two hashes; skills of a sync land in
  `skills.disabled`.
- import: plan from a source tree with a duplicate real path and an existing name;
  copy skips symlinks inside the skill; delete of an imported symlink removes the link
  and leaves the target.
- API validation (`skills_api.rs`): invalid name, reserved name, oversized body -> 400;
  a name the owner has already (POST), a change or a delete of a bundled skill -> 409;
  `/<name>` of a message names an enabled skill only.
