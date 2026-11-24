# Spec: web tools: fetch and search (S30)

The agent can read a public web page and search the web without driving the browser:
far fewer tokens, and no window. Tools: tools.md section 20, group `web`. Both live in
`kagentd` (it already has reqwest), like the `screen` tool.

## 1. Egress guard (pure: `kagentd/src/web/guard.rs`)

- Only `http` and `https`. No `user:password@` part. A host is required.
- Blocked hosts: `localhost`, names ending in `.localhost`, `.local`, `.internal`; IP
  literals that are loopback, private (10/8, 172.16/12, 192.168/16), link-local
  (169.254/16, fe80::/10), unique local (fc00::/7), multicast, unspecified, broadcast,
  shared address space (100.64/10); IPv4-mapped IPv6 addresses of the same ranges.
- The host is resolved before the request (`tokio::net::lookup_host`). Every resolved
  address must pass the same check; one blocked address refuses the request.
- Redirects are followed by hand, at most 5, and every target is checked again. A
  blocked target stops with "redirect to a blocked address stopped".
- Why: the daemon's own API, the Ollama server and the DevTools port (no
  authentication, runs scripts) listen on loopback.

## 2. `web_fetch`

- Request: GET, `User-Agent: KAgent/<version>`, `Accept:
  text/html,application/xhtml+xml,application/json,application/xml,text/*;q=0.9`, no
  cookies, no credentials, 15 s total timeout, 2 MiB body cap (the stream is stopped at
  the cap).
- HTML to text (`web/html.rs`, pure, written by hand, no dependency):
  - `script`, `style`, `noscript`, `template`, `svg` and `iframe` elements are dropped.
  - When `<main>` or `<article>` exists, only its content is used.
  - Block elements become line breaks; whitespace is collapsed; the common entities are
    decoded (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#39;`, `&nbsp;` and numeric forms).
  - The title comes from `<title>`. Links: text and `href` of every `<a href>`, at most
    30, relative links resolved against the final URL.
- Content types: HTML -> extracted; `text/*`, `application/json`, `application/xml`,
  `application/yaml` -> the raw text; anything else -> error "not a text page (type,
  size)".
- Cache: 15 minutes, 50 entries, keyed by final URL and `max_chars`.
- Assessment: read-only, side effect `Network`, timeout 20 s.

## 3. `web_search`

Engines, by `settings.search.engine`:

| Engine | Request | Results |
|---|---|---|
| `duckduckgo` (default, no key) | POST `https://html.duckduckgo.com/html/` with `q=<query>` | `a.result__a` (title, href with the `uddg=` redirect parameter decoded) and `a.result__snippet` |
| `brave` | GET `https://api.search.brave.com/res/v1/web/search?q=<query>&count=<limit>`, header `X-Subscription-Token` | `web.results[]`: `title`, `url`, `description` |
| `searxng` | GET `<searxUrl>/search?q=<query>&format=json` | `results[]` |
| `none` | | the tool answers that search is off |

- The Brave key lives in Secret Service (`kind=search, id=brave`), with the usual file
  fallback.
- Output: `[n] title - url` and the snippet per result.
- Failure: an error with the cause (rate limit, bad key, unreachable, no results).
- Assessment: read-only, side effect `Network`, timeout 15 s.

## 4. Prompt

The working rule that tool output is data covers both tools; their output is labelled
untrusted (tools.md section 20).

## 5. UI: Settings -> Web

- Search engine: DuckDuckGo, Brave Search API, SearXNG, Off.
- Brave key (stored in Secret Service) and SearXNG URL, shown when they apply.

## 6. Tests (no network)

- Guard table: 25 URLs and addresses, accepted and blocked, including IPv4-mapped IPv6
  and a name that resolves to a blocked address (the resolver is a trait with a fake);
  a redirect chain that ends at a blocked address.
- HTML extraction on saved real pages under `crates/kagentd/src/web/fixtures/`: a news
  article, a GitHub README, a page with `<main>`, a page with entities, a page with
  bytes that are not UTF-8 -> the expected text and links.
- A saved DuckDuckGo results page -> results; a challenge page -> the error.
- Brave and SearXNG JSON fixtures.
- Cache behaviour.

## 7. Not in scope

Provider-native search tools, PDF extraction, pages behind a login (use `browser`),
images.
