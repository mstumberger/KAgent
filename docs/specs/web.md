# Spec: web tools: fetch and search (S30)

The agent can read a public web page and search the web without driving the browser:
far fewer tokens, and no window. Tools: tools.md section 20, group `web`. Both live in
`kagentd` (`crates/kagentd/src/web/`: it has reqwest), like the `screen` tool. Every
request goes through one transport (`web/mod.rs`: `Transport`, `Request`, `Answer`,
`Fail`), which the tests replace with a loopback fake.

## 1. Egress guard (pure: `kagentd/src/web/guard.rs`)

- Only `http` and `https`. No `user:password@` part. A host is required.
- Blocked hosts: `localhost`, names ending in `.localhost`, `.local`, `.internal`; IP
  literals that are loopback, private (10/8, 172.16/12, 192.168/16), link-local
  (169.254/16, fe80::/10), unique local (fc00::/7), multicast, unspecified, broadcast,
  shared address space (100.64/10); IPv4-mapped IPv6 addresses of the same ranges. The
  brackets of an IPv6 host are stripped before the check. Not blocked: fec0::/10
  (site-local, deprecated), 198.18.0.0/15 and 240/4.
- The host is resolved before the request (behind a small resolver trait; in the daemon
  `tokio::net::lookup_host`). Every resolved address must pass the same check; one
  blocked address refuses the request.
- The resolved addresses are pinned into the client of that request: the name is not
  looked up a second time (a name that answers a public address to the guard and a
  private one to the client would pass otherwise).
- Redirects are followed by hand, at most 5, and every target is checked again. A
  blocked target stops with "redirect to a blocked address stopped". No search engine
  follows a redirect.
- `searxUrl` is the owner's setting and may be a local instance: no address rule. It
  must be an http or https address with a host and without a password (else 400, and
  refused on load).
- Why: the daemon's own API, the Ollama server and the DevTools port (no
  authentication, runs scripts) listen on loopback.

## 2. `web_fetch` (`web/fetch.rs`)

- Request: GET, `User-Agent: KAgent/<version>`, an `Accept` header for text, no
  cookies, no credentials, 15 s total timeout, 2 MiB body cap (the stream is stopped at
  the cap).
- HTML to text (`web/html.rs` with `html/{tags,entities,charset}.rs`, pure, written by
  hand, no dependency):
  - `script`, `style`, `noscript`, `template`, `svg` and `iframe` elements are dropped.
  - When `<main>` exists, only its content is used; else the first `<article>`; a
    `<main>` that says nothing is not the page.
  - Block elements become line breaks; whitespace is collapsed; the common entities are
    decoded. A row of a table stays on one line, its cells a space apart.
  - The title comes from `<title>`.
  - Links: the text and `href` of the `<a href>` elements of the part of the page that
    is read, http and https only, at most 30, relative links resolved against the final
    URL. Left out: a fragment of the page, a link without text (the `alt` of its image
    is used when there is one), and a second link to one address.
- Charsets: a body that is not UTF-8 is read by the charset of its `Content-Type`, else
  of its `<meta charset>`: windows-1252 and windows-1250 are decoded, UTF-16 by its
  mark, any other charset with U+FFFD. A body with no type is a page when it starts
  like one, else text.
- Content types: HTML -> extracted; `text/*`, `application/json`, `application/xml`,
  `application/yaml` -> the raw text; anything else -> error "not a text page (type,
  size)".
- `max_chars` outside 1000..20000 is read as the nearest end. An answer is at most
  23 500 bytes: the links go first, then the text.
- Cache: 15 minutes, 50 entries, keyed by URL and `max_chars`; a page reached through
  redirects is kept under the address asked for as well.
- Assessment: read-only, side effect `Network`, timeout 20 s.

## 3. `web_search` (`web/search.rs`)

Engines, by `settings.search.engine`:

| Engine | Request | Results |
|---|---|---|
| `duckduckgo` (default, no key) | POST `https://html.duckduckgo.com/html/` with `q=<query>` | `a.result__a` (title, href with the `uddg=` redirect parameter decoded) and `a.result__snippet`; blocks marked `result--ad` are left out |
| `brave` | GET `https://api.search.brave.com/res/v1/web/search?q=<query>&count=<limit>`, header `X-Subscription-Token` | `web.results[]`: `title`, `url`, `description` |
| `searxng` | GET `<searxUrl>/search?q=<query>&format=json` | `results[]` |
| `none` | | the tool answers that search is off |

- The Brave key lives in Secret Service (`app=kagent, kind=search, id=brave`), file
  fallback `<config>/search.secret` when `KAGENT_HOME` is set. It is set with
  `PUT /api/web/brave-key`, stored after a shape check only (16 to 128 printable ASCII
  characters without a space); a wrong key shows at the first search. The API never
  returns the key, only `configured`.
- A DuckDuckGo challenge is a status of 202 or 403, or the words of one; a status 200
  with neither results nor a known marker is "answered a page this tool cannot read".
  DuckDuckGo limits a client that sends many searches; the tool then says so.
- Output: `[n] title - url` and the snippet per result. `limit` outside 1..10 is read
  as the nearest end; a query over 400 characters is an error.
- Failure: an error with the cause (rate limit, bad key, unreachable, no results).
- Assessment: read-only, side effect `Network`, timeout 15 s.

## 4. Prompt

The working rule that tool output is data covers both tools; their output is labelled
untrusted (tools.md section 20).

## 5. UI: Settings -> Web (Basics)

- Search engine: DuckDuckGo, Brave Search API, SearXNG, Off.
- Brave key field (stored in Secret Service; `configured` is shown, never the key) and
  the SearXNG URL, shown when they apply. The types and the client are in
  `web/src/settings/web/api.ts`.

## 6. Tests (no network)

- Guard table: 25 URLs and addresses, accepted and blocked, including IPv4-mapped IPv6
  and an IPv6 host in brackets, and a name that resolves to a blocked address (the
  resolver is a trait with a fake); a redirect chain that ends at a blocked address.
- HTML extraction on saved real pages under `crates/kagentd/src/web/fixtures/`: an LWN
  article, a GitHub README, a page in windows-1250, a page with entities, the page of
  kernel.org: the expected text and links.
- A saved DuckDuckGo results page -> results (the `uddg=` form with an inline sample of
  the older markup, because the page of 2026-11-26 holds direct addresses); a
  hand-written challenge page -> the error; hand-written Brave and SearXNG JSON
  fixtures.
- The cache; the body cap and the timeout against the loopback fake.

## 7. Not in scope

Provider-native search tools, PDF extraction, pages behind a login (use `browser`),
images.
