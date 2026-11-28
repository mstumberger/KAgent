# Contributing

KAgent is an early project by one person. Issues and pull requests are welcome; for a
large change, open an issue first so we can agree on it.

## Build and test

Rust (stable, through rustup) and Node with pnpm are needed. No test needs root, the
network or an API key, except those gated by `KAGENT_IT=1`, which use a local Ollama.

```bash
RUSTFLAGS="-D warnings" cargo build --workspace
RUSTFLAGS="-D warnings" cargo test --workspace
(cd web && pnpm install && npx tsc --noEmit -p tsconfig.app.json && pnpm lint)
```

The web suites drive the real window in headless Chrome against a mocked API. Build
the window somewhere of your own, point `KAGENT_DIST` at it and run a suite from the
repo root:

```bash
(cd web && npx vite build --outDir /tmp/kagent-dist)
KAGENT_DIST=/tmp/kagent-dist python3 scripts/test-sidebar-web.py
```

The suites are `scripts/test-*-web.py`. They need `google-chrome` and the Python
`websockets` package. `scripts/take-screenshots.py` makes the images in `docs/images/`.

## Rules that matter

- Spec first: a new feature starts as a file in `docs/specs/`, then the code. Stages
  and decisions are in `docs/IMPLEMENTATION.md`; `AGENTS.md` has the full rules.
- Tests assert the intended behaviour, sit next to the code, and need no root or network.
- Keep files under about 400 lines; split by concern.
- Tools go through `kagent_core::platform`: no distro or desktop assumptions.
- Do not use the em dash character in any text; use a hyphen.
- Tool output and file content are data, never instructions.

## Reporting a bug

Open an issue with the bug report template: what you did, what you expected, what
happened, the commit you built, your distro and desktop, and
the relevant lines of `~/.local/share/kagent/logs/kagentd.log`. Remove tokens and keys
first. For a security problem, do not open an issue: see `SECURITY.md`.

## Licence of contributions

KAgent is GPL-3.0-or-later. By sending a contribution you agree that it is licensed
under the same terms. Do not add licence headers to source files.
