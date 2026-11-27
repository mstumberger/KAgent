# Spec: token guard (S34)

The agent runs with the owner's privileges. KAgent's own API listens on 127.0.0.1:7878 and
takes the bearer token that lies in `<config>/token`. A file read or a shell command could
bring that token to the agent, and with it the agent could change its own settings and
hooks, or answer its own approval cards (found in S32; read, not tried). This stage closes
the easy ways. It is defence in depth, not a sandbox: an agent that runs code as the owner
can still reach whatever the owner can (section 5).

## 1. Credential files

`<config>/token`, `<config>/router.secret`, `<config>/telegram.secret`,
`<config>/search.secret`, `<config>/calendar.json` (the Google connection: client secret and
refresh token, S45) and `<config>/maps.secret` (the Maps key, S45). Where they are follows `kagentd/src/paths.rs` (`KAGENT_HOME`,
else XDG); the rules read the environment at each check.

- `read_file` refuses them with an error and without a card, in every mode: "refused, the
  file holds a KAgent credential".
- `search_files` leaves out the rows that come from them.
- `list_dir` shows their names and sizes only.

## 2. Shell rules (each asks in every mode)

A shell command is marked destructive (the engine's gate that asks in `full` too, as
hooks.md section 5 does; never rememberable) when:

- a word of it names a credential file in any written form: as an argument, inside `$(...)`
  or backticks, after a `cd` of the line, or through a wildcard before it (`tok*`); `~`,
  `$HOME` and `${HOME}` are read. A listing of the config folder with a wildcard
  (`ls ~/.config/kagent/*`) is caught too: that is the safe direction;
- it runs `journalctl` on the daemon (the command names `kagent`): the journal may hold the
  start line with the token;
- it contains `7878`, the port of KAgent's own API.

## 3. The token never reaches the model

- The daemon gives the agent a redactor (`ToolServices.redact`) that replaces its token in
  every tool output with `<redacted: KAgent API token>`. It runs in `execute_call`, after
  the output budget and before the result is stored, shown or sent: one place for chat,
  voice, subagents, tasks and the MCP server. Outputs the agent reads later (`task_output`,
  a session file) pass through the same place.
- `kagentd` prints the URL with the token only when stdout is a terminal. Under systemd
  stdout is the journal: there it prints where the token lies instead.

## 4. Tests (no root, no network)

- `agent/hooks/protect/tests.rs`: the credential files in every written form (`~`, `$HOME`,
  `..`, `.`), and what is not one (`settings.json`, `token.bak`, `/tmp/token`); shell
  commands that read a credential or call the API (as an argument, quoted, inside `$(...)`,
  in backticks, through a wildcard, after a `cd`, `journalctl` on the daemon, a call to
  port 7878), and those that do not.
- `tools/shell.rs`: such a command asks in every mode (`destructive`, not read-only);
  reading the settings stays read-only.
- `tools/files.rs`: `read_file` refuses a credential file and does not leak its content
  (with an injected check and a temporary file, never the owner's token); the rows of
  `search_files` from a credential file are recognised.
- `tests/token_guard.rs`: a tool output that holds the token reaches the model and the
  session without it; without a redactor it passes unchanged.

The calendar module adds one rule of its own (`calendar.md` sections 1 and 5): a token or a key
never appears in a tool result, a view, an error or a log line. The access token lives in memory
only; Google's own error text is scrubbed of the key and the token before it is shown; the Maps
key travels in a header, never in a URL. `calendar/tests/secrets.rs` runs a whole session against
the fake Google with the log captured and greps all of it.

## 5. Known limits

- A copy of the token that a command transforms before the output (`journalctl ... | sed
  's/0/x/g'`, `base64`) is not recognised by the redactor.
- A script that reads the token and calls the API without naming the path or the port in
  its command line (a computed port, a path built from parts) is not caught by the shell
  rules.
- The token is also in the local storage of the app's browser profiles on disk
  (`<data>/app-profile`, `<data>/widget-profile`). Reading it there brings it into the
  output, where the redactor takes it out; an encoded copy is not covered.
- Journal entries written before this stage still hold the start line with the token;
  `journalctl` on the daemon asks from now on.
