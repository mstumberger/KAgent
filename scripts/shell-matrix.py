#!/usr/bin/env python3
"""H1 of the shell (docs/specs/shell.md section 3): every view and every settings page at 1920x1080, 1200x820, 700x500 and the widget
(340x460), dark and light, with screenshots in /tmp/kagent-v003-shell-shots/<out>/ and the findings measured per page.

  scripts/shell-matrix.py [--out before|after] [--themes dark,light] [--pages chat,settings-models] [--no-shots]

Prints the findings per page and size, and the table of docs/specs/shell.md section 8 (pages ordered by their findings). Run from the repo
root with KAGENT_DIST set to a build of web/ (`npx vite build --outDir <dir>`). Exit code 1 when there is a finding."""
import argparse, asyncio, json, os, sys
import picker_web_lib as lib
import shell_matrix_lib as M

ap = argparse.ArgumentParser()
ap.add_argument("--out", default="before")
ap.add_argument("--themes", default="dark,light")
ap.add_argument("--pages", default="")
ap.add_argument("--no-shots", action="store_true")
args = ap.parse_args()


async def main():
    root = f"/tmp/kagent-v003-shell-shots/{args.out}"
    os.makedirs(root, exist_ok=True)
    only = [p for p in args.pages.split(",") if p]
    pages = [p for p in M.PAGES if not only or p[0] in only]
    async with lib.Env("kagent-shell", root) as env:
        result = await M.run_matrix(env, root, tuple(args.themes.split(",")), pages, shots=not args.no_shots)
    json.dump({"|".join(k): v for k, v in result.items()}, open(f"/tmp/kagent-v003-shell-{args.out}.json", "w"), indent=1)
    # One line per page: the sizes and kinds that have findings (a theme only when it differs).
    rows = {}
    for (theme, page, size), found in sorted(result.items()):
        for f in found:
            rows.setdefault(page, {}).setdefault(f["kind"], set()).add(size)
    total = sum(len(v) for v in result.values())
    for page, kinds in sorted(rows.items(), key=lambda kv: -sum(len(s) for s in kv[1].values())):
        print(f"{page:24} " + "; ".join(f"{k}: {','.join(sorted(s))}" for k, s in sorted(kinds.items())))
    for size, kinds in sorted(M.count(result).items()):
        print(f"{size:10} {sum(kinds.values()):4} findings  {kinds}")
    print(f"TOTAL findings: {total}")
    return 1 if total else 0


sys.exit(asyncio.run(main()))
