# Security policy

## Reporting a vulnerability

Please report it privately, not in a public issue. Use GitHub's private vulnerability
reporting: on the repository page open the Security tab and choose "Report a
vulnerability". Say what you found, how to reproduce it, and what it lets an attacker do.

This is a one-person project. I will answer as soon as I can and keep you told of the
fix. Please give me time to fix it before you publish.

## In scope

- The daemon's HTTP and WebSocket API: authentication (the bearer token, the Host check),
  and anything reachable without the token.
- The root helper `kagent-priv`: any way to run something it should refuse, or to run as
  root outside its fixed operations.
- The permission engine: a change that runs without the approval it should need, or a
  mode that lets through more than it says.
- The token guard: a way for the agent to read the API token or other credential files,
  or to answer its own approval cards.

## Out of scope

- A model giving a wrong answer, or being misled by text it reads. Report it as a bug.
- Anything that needs you to run your own code as the same user: the agent runs with your
  privileges and the token guard is not a sandbox (see the README).
- `full` permission mode doing what it says.
