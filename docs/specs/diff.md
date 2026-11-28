# The diff view

Status: v0.0.3, stage I2 (plan `plans/tickets-and-steering.md` section 3, local). The daemon computes the diff; the web part
only renders it. Comments on lines and "Approve" belong to the workflow stage and are not built.

## 1. The model

```json
{"files": [{"path": "src/a.rs", "oldPath": null, "status": "modified", "binary": false, "tooLarge": false,
            "added": 3, "removed": 1,
            "hunks": [{"oldStart": 10, "oldLines": 4, "newStart": 10, "newLines": 6, "header": "fn main()",
                       "lines": [{"kind": "ctx", "old": 10, "new": 10, "text": "..."},
                                 {"kind": "del", "old": 11, "new": null, "text": "...", "ranges": [[4, 9]]},
                                 {"kind": "add", "old": null, "new": 11, "text": "...", "ranges": [[4, 12]]}]}]}],
 "added": 3, "removed": 1, "truncated": false}
```

- `status`: `added`, `modified`, `deleted`, `renamed` (`oldPath` set; a rename with changes has hunks).
- A line: `kind` `ctx`, `add` or `del`; `old` and `new` are the one-based line numbers on each side (`null` on the side
  where the line is not); `text` has no line terminator. `ranges` (only on `add` and `del` lines that pair with a line of
  the other kind in the same change block) are `[start, end)` positions in **characters** of `text` that differ from the
  paired line; a block with a different number of removed and added lines pairs them in order, and the rest have none.
- Three lines of context. Hunks are what the view shows; the lines between hunks are "collapsed unchanged regions": the view
  shows "N unchanged lines" with an expand button that asks for nothing more when the file text is also sent (section 3).
- A **binary** file (git says so, or the text has a NUL byte) and a file whose text is over 1 MB (each side for a text diff;
  the git patch of one file for a repository) carry only `path`, `status`, `binary` or `tooLarge` and the line counts
  (`added`, `removed`: `null` for a binary file), with no `hunks`.
- The total patch read from git is capped at 8 MB: more sets `truncated: true` and drops the files that do not fit (their
  names stay with `tooLarge`). At most 500 files; the rest set `truncated`.

## 2. The line diff (`kagentd/src/diff/lines.rs`)

In process, no dependency: the common prefix and suffix are cut off, then a longest-common-subsequence over the middle by
dynamic programming (at most 4 million cells; a larger middle is shown as all removed then all added). Line endings: `\n` and
`\r\n` both split lines; the `\r` is not part of `text`. Intra-line `ranges`: the common prefix and suffix of the two paired
lines are cut off, the middle is the range (so a changed word, or an inserted piece, is marked).

## 3. Journal changes: `GET /api/changes/:id/diff`

A journal change of a file edit (`write_file`, `edit_file`; `op.action` `write_file` or `edit_file`, `undo.action`
`restore_file`) has a stored **before** (the backup the undo restores) and a stored **after**: from this stage the two tools
also write the text they wrote to `<data>/changes/aft_<uuid>` and record its path in the `op` as `afterBackup`. A record
from before that (no `afterBackup`) uses the file as it is now, and the answer says `after: "now"`. The answer:

```json
{"id": "chg_...", "path": "/home/me/a.txt", "after": "stored" | "now", "before": "<text>" , "afterText": "<text>", "diff": {Diff}}
```

`before` and `afterText` (each at most 1 MB, else `null` and `diff` is a summary) let the view expand the collapsed regions
without another request. Errors: 404 for an unknown change; 409 `conflict` "this change has no stored copy of the file" (a
change that is not a file edit, a root-helper edit, a backup that is gone). The route reads the journal and the two files
only: it never writes.

## 4. A repository: `POST /api/diff`

Body (`deny_unknown_fields`): `{"project": "<path>", "from": "<ref>", "to"?: "<ref>"}`.

- `project` must be **one of the projects list** (`xdg::projects` over `settings.desktop.projectRoots`, as `GET /api/projects`
  lists them), compared after `canonicalize`; else 400 "not a known project". It must be a git work tree (`git rev-parse
  --is-inside-work-tree`), else 409 `conflict` "not a git repository".
- A ref is validated before git sees it: 1 to 200 characters of `A-Z a-z 0-9 . _ / ~ ^ @ { } -`, not starting with `-`, no
  `..`, no `@{` followed by anything but `u`, `upstream`, `push` or digits and `}`. Anything else is 400 (`a ref cannot
  start with '-'`, ...). Then `git rev-parse --verify --quiet <ref>^{commit}` with `--end-of-options`; a ref that does not
  resolve is 400 "unknown ref".
- With `to`: the changes from `from` to `to` (`git diff from to`). Without it: the **working tree** against `from`.
- Everything runs through the one exec helper (`kagent_core::tools::exec`, a timeout of 20 s, `git -C <project>`, never a
  shell, `GIT_OPTIONAL_LOCKS=0`, `--no-ext-diff --no-textconv --no-color -M`, `-c core.quotepath=false`), read-only: no
  `git add`, `checkout`, `reset`, `stash` or any write; the daemon never writes into the repository. The commands are
  `rev-parse`, `diff --numstat`, `diff -U3`.
- The parser reads the unified output: `diff --git` blocks, `new file mode`, `deleted file mode`, `rename from/to`,
  `Binary files ... differ`, `@@ -a,b +c,d @@ header`, `\ No newline at end of file`. A path with spaces or a non-ASCII name
  works (no quoting, `core.quotepath=false`).

The answer is `{"project", "from", "to", "diff": Diff}`.

## 5. The component `DiffView` (`web/src/components/diff/`)

- Props: a `Diff`, optionally the two texts of each file, `onOpenEditor(path, line)`, `onRevert(path)`.
- Left: a **file tree** (folders collapsed by their common prefix), each file with a mark (`A`, `M`, `D`, `R`) and `+n -m`;
  a click scrolls to the file; the focused file is marked `aria-current`.
- Right: per file a header (path, counts) and its hunks as **side by side** (two columns with line numbers; a changed pair
  on one row, an addition or a removal against an empty cell) or **unified**. A toggle **Side by side | Unified**, remembered
  in this browser (`localStorage`, wrapped in try/catch). Intra-line ranges are drawn as a stronger background of the
  characters. Colours work in dark and light and never carry the meaning alone (a `+` and a `-` sign in the gutter).
- Collapsed unchanged regions between hunks and before the first and after the last: "N unchanged lines", a button to show
  them (needs the texts; without them the row says so and has no button).
- **Next and previous change**: `F7` and `Shift+F7` (when the view has focus, or the pointer is over it) and two buttons
  with labels; they move the focus to the next or previous change block across files and show "change 3 of 12".
- No syntax colours: `web/package.json` has no highlighter and none is added.
- Binary and too-large files show their summary line ("binary file, not shown", "over 1 MB, not shown").
- Actions: **Open in editor** on a line (the `open_in_editor` path, `POST /api/ui/tools/open_in_editor/call {path, line}`), and
  **Revert file** on a journal change (the existing undo, `POST /api/changes/:id/undo`, which asks). Comments and Approve are not
  built.

## 6. Where it is used

- The **Changes** view (`web/src/views/Changes.tsx`): a file edit change has a button **Diff** that opens the view with
  `GET /api/changes/:id/diff` under the row, with Revert file. Other changes have no button.
- A ticket's detail pane (`tickets.md` section 6): the tab **Changes** shows `POST /api/diff` for the ticket's first `range`
  link (`from..to`: `from` and `to`) or `commit` link (`<commit>^` and the commit); a ticket in a project without a git
  repository, or with no such link, says "Link a branch or commit range to see its changes." with a field to add one. The
  project of the request is the ticket's project (an Inbox ticket has none: the tab says so).

## 7. Files

`crates/kagentd/src/diff/{mod.rs (the model), lines.rs, git.rs (refs, exec, the parser), http.rs, tests*.rs}`;
`web/src/components/diff/{DiffView.tsx, Tree.tsx, Side.tsx, model.ts, diff.css}`.

## 8. Tests

The line diff on fixed pairs (insert, delete, change, CRLF, a middle over the cell limit, identical texts); the intra-line
ranges; the git parser on real output samples; the ref validation (a ref starting with `-`, with `..`, with a space, an
unknown ref); `POST /api/diff` on temporary repositories created by the test (two commits, a rename, a deletion, a binary
file, the working tree, a file over 1 MB); a project that is not in the list; a journal change with and without
`afterBackup`; and `scripts/test-diff-web.py`.
