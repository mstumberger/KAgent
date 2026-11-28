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

- Input: `url`, `max_chars`, and `render` (boolean, default false: read through the reader).
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
- `render: true` reads the page through the reader (section 8) instead of plain HTTP. The
  reader is also the fallback when the plain answer is a challenge page (a 403, 429 or
  503 status, or a 200 page that `challenge::is_challenge` recognises) or an empty shell
  that needs JavaScript (under 200 characters of text and at least one `<script>`), and
  the first choice when the host is on `web.readerSites`. The text comes from the same
  extraction, with the same limits; the cache key is the same.
- Assessment: read-only, side effect `Network`, timeout 45 s (a read through the reader
  may wait up to 20 s for a challenge to clear).

## 3. `web_search` (`web/search.rs`)

Engines, by `settings.search.engine`:

| Engine | Request | Results |
|---|---|---|
| `duckduckgo` (default, no key) | POST `https://html.duckduckgo.com/html/` with `q=<query>` | `a.result__a` (title, href with the `uddg=` redirect parameter decoded) and `a.result__snippet`; blocks marked `result--ad` are left out |
| `brave` | GET `https://api.search.brave.com/res/v1/web/search?q=<query>&count=<limit>`, header `X-Subscription-Token` | `web.results[]`: `title`, `url`, `description` |
| `searxng` | GET `<searxUrl>/search?q=<query>&format=json` | `results[]` |
| `reader` | the reader (section 8) reads `https://html.duckduckgo.com/html/?q=<query>` as a page | the same parser as `duckduckgo`, on the page the browser rendered |
| `none` | | the tool answers that search is off (the old single setting; see 3.1) |

- The Brave key lives in Secret Service (`app=kagent, kind=search, id=brave`), file
  fallback `<config>/search.secret` when `KAGENT_HOME` is set. It is set with
  `PUT /api/web/brave-key`, stored after a shape check only (16 to 128 printable ASCII
  characters without a space); a wrong key shows at the first search. The API never
  returns the key, only `configured`.
- A DuckDuckGo challenge is a status of 202 or 403, or the words of one; a status 200
  with neither results nor a known marker is "answered a page this tool cannot read".
  DuckDuckGo limits a client that sends many searches; the tool then says so.
- Output: `[n] title - url` and the snippet per result, under a line that names the engine
  that answered (`Results of DuckDuckGo for "q":`, with ` (from the cache)` when it did not
  ask again). `limit` outside 1..10 is read
  as the nearest end; a query over 400 characters is an error.
- Failure: an error with the cause for one engine; with the chain (3.1), `not done` with
  what each engine said and what to do.
- Assessment: read-only, side effect `Network`, timeout 45 s (a burst waits in a queue).

### 3.1 The chain: a search that does not fall over (R1)

- **Settings.** `search.engines` is the ordered chain: a list of `duckduckgo`, `brave`,
  `searxng`, `reader`, each at most once (the plan called it `web.searchEngines`; it
  lives in the `search` section next to the keys it needs). Absent (`null`, the default):
  the chain is derived from the old `search.engine` (a migration, no file is rewritten):
  that engine first, then the default fallbacks `duckduckgo` and `reader`, without
  repeats; `engine: "none"` is an empty chain. An empty list `[]` is "search off". `none`
  is not a chain entry. `engine` stays readable and writable and is ignored once
  `engines` is set. Both `search.engines` and `web.*` are protected keys (what leaves the
  machine and where pages are read from): the owner changes them on the screen only.
- **Cache.** A query is normalised (trimmed, lowercased, whitespace collapsed); the
  results of up to 10 hits are kept 15 minutes, 50 entries, whichever engine answered; a
  call with a smaller `limit` cuts them. The cache answers without a request and without
  pacing.
- **Pacing per engine, with a queue.** One request to an engine at most every: DuckDuckGo
  3 s, the reader (also DuckDuckGo) 5 s, Brave 1 s, SearXNG 0. A request takes the next
  free slot of its engine and waits for it; a wait over 20 s counts as "busy": that
  engine is skipped for this search, without a cooldown. A burst of five searches
  therefore all answer, 3 s apart, instead of failing.
- **Cooldowns.** An engine that answered a challenge, a rate limit (429, 403), an
  unreadable page, a bad key or did not answer goes on a cooldown of 5 minutes; the next
  failure while it is not yet forgiven doubles it (10, 20, 40 minutes, at most 1 hour). A
  success resets it. An engine on a cooldown is skipped. A missing Brave key or SearXNG
  address skips the engine and starts no cooldown. "No results" is an answer, not a
  failure: it stops the chain.
- **Result.** The first engine that answers is named in the result. When every engine
  failed or was skipped: `not done: web search failed (DuckDuckGo: ...; Brave: no key)`,
  next: "do not search again now; use web_fetch on an address you know, or tell the owner
  to look in Settings -> Web (engines resume after their cooldown)".
- **Burst refusal.** A fourth search of one session within 10 s is refused before it
  runs (`not done: a fourth search within 10 seconds was not run`, next: "read the
  results you already have first; several searches at once are not faster"). A session is
  the key because a tool call carries no turn id. The chat prompt says so too.
- The clock is a trait (`web/chain.rs: Clock`): the tests use a fake whose `sleep`
  advances it.

## 4. Prompt

The working rule that tool output is data covers both tools; their output is labelled
untrusted (tools.md section 20). The chat prompt adds, when `web_search` is there, one
sentence: never send several web searches at once, they are not faster (the burst refusal
of 3.1 stands behind it; the rules table row).

## 5. UI: Settings -> Web (Basics)

- Search engines: the chain of 3.1, in order, each with a switch and move up and down;
  the engines that are off are listed under it. All off is "Off".
- The reader (section 8): on or off, the browser it found (or "none found"), "always use
  the reader for" (hosts), the warmed sites (section 9) with Forget, and "Clear the
  reader's profile".
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
- The chain, with fake engines and a fake clock: a burst of five searches all answer; a
  challenge moves down the chain and cools that engine; the cooldown doubles and ends;
  the cache answers without a request; the fourth search in 10 s is refused; an old
  settings file with `engine` alone loads and means that engine plus the fallbacks.
- The reader, against a loopback fake site and a real headless browser (skipped, with a
  printed reason, when none is installed): a page that needs JavaScript is read; a public
  page that redirects to `127.0.0.1` and one to `169.254.169.254` are refused inside the
  browser (the fake site is on loopback: the test egress lets the fake's own address
  through and nothing else); a page that asks to download or opens a dialog changes
  nothing; the profile is the reader's, never a browser's own; the browser is gone after
  the idle time; a challenge page gives `not done` and the notice; after the owner's
  warm-up (the fake sets a cookie) the read succeeds.
- `challenge::is_challenge`: a table of recorded interstitial titles (English, Slovenian)
  and of normal pages that must not match.

## 7. Not in scope

Provider-native search tools, PDF extraction, pages behind a login (use `browser`),
images.

## 8. The reader: a headless browser of KAgent's own (R2)

The reader is for reading pages, not for acting. It is not browser attach (`browser.md`,
the owner's visible browser, for acting and testing) and not the `own` window of the
`browser` tool. `web/reader/`.

- **Which browser.** The first of the list the `browser` tool already uses (`brave-browser`,
  `google-chrome`, `google-chrome-stable`, `chromium`, `chromium-browser`,
  `microsoft-edge`) that is on `PATH`. None: `web_fetch` says so once in its result ("the
  reader could not run: no Chromium-family browser was found") and the plain answer is
  still given; the plain HTTP path never depends on the reader.
- **Profile.** `<data>/reader-profile`. Never the owner's browser profile, never the
  `browser` tool's `<data>/browser-profile`, never the app window's: a function builds the
  path and a test checks it. The owner logs in to the sites he wants read in the window
  of section 9, nowhere else.
- **Start.** On demand: `--headless=new`, `--remote-debugging-port=0` on 127.0.0.1 (the
  port is read from `DevToolsActivePort` of the profile), `--user-data-dir`,
  `--no-first-run`, `--no-default-browser-check`, `--password-store=basic`,
  `--mute-audio`, `--disable-extensions`; in its own systemd scope when the daemon runs
  under systemd (`appwin/scope.rs`), so a stop of the daemon unit does not crash it. The
  DevTools port needs no authentication and runs scripts: it listens on loopback and
  lives only as long as the browser does. Driven with the daemon's CDP client
  (`browser/cdp.rs`, extended with `send` and `recv` so events are read).
- **One page at a time.** A read takes the reader's lock; another waits (within the
  tool's timeout). A read opens a tab, closes it after, and reuses the browser.
- **Close.** After 2 idle minutes, and at daemon shutdown (`Web::shutdown`).
- **Reading.** `Page.navigate`, wait for the load event, then for network quiet (no
  request in flight for 500 ms, at most 5 s more); the text is taken from the rendered
  `outerHTML` by the same HTML-to-text code (`html::to_text`), with the same size limits (a
  document over 2 MiB is `TooLarge`). A status of 400 or more of the main document is
  the same failure as in plain HTTP. The whole read is limited to 40 s.
- **The egress guard inside the browser.** `Fetch.enable` with the pattern `*` pauses every
  request, the navigation, each redirect and each subresource: the URL goes through the
  same `Egress::admit` as every other web request; only http, https, `data:`, `blob:` and
  `about:` are let on (`file:`, `chrome:` and the rest fail), and a request the egress
  refuses is failed with `BlockedByClient`. A refused main document ends the read:
  `refused: ...` or `redirect to a blocked address stopped: ...`. Decisions are kept
  per origin for the read. Limits, said plainly: the browser resolves a name itself, so
  a name that changes its answer between the guard's lookup and the browser's (DNS
  rebinding) is not pinned as it is in the plain client; WebSocket connections do not
  pass through `Fetch`. Reading is still done from a profile with no login of the
  owner's, and nothing local can be changed by a GET.
- **What it never does.** No click, no typing, no form, no `Input.*` calls. Downloads are
  denied (`Browser.setDownloadBehavior` deny); a JavaScript dialog is dismissed
  (`Page.handleJavaScriptDialog` accept false); a new window opened by the page is closed
  (`Target.setDiscoverTargets`: a target that is not ours is closed).

## 9. Challenges and warm-up (R3)

- **Recognising one.** `web/reader/challenge.rs`, pure: a page is a challenge when its
  title is one of the known interstitial titles (English and Slovenian: "Just a
  moment...", "Attention Required", "Verify you are human", "Security check", "Checking
  your browser", "Access denied", "Samo trenutek ...", "Preverjanje varnosti", "Potrdite,
  da ste človek" and the like) or when a short page (under 2000 characters of text) holds
  a marker (`cf-challenge`, `challenge-platform`, `captcha`, "verify you are human",
  "unusual traffic", "are you a robot", "preverjamo vaš brskalnik"). A long article that
  mentions a captcha is not one. The test table lists recorded titles and normal pages
  that must not match.
- **Waiting.** The reader waits up to 20 s for a challenge to clear by itself (the page
  reloads when it does). If it does not: the result is `not done: this site asks for a
  human check`, next: "tell the owner; a notice offers to open the page in a window where
  he can pass the check once", and a notice goes out: `kind` error, key
  `reader:check:<host>`, text "<host> asks for a human check", with an action
  `{label: "Open it", path: "/api/web/reader/open", body: {url}}` (notices.md).
- **Open it.** `POST /api/web/reader/open {url}` closes the headless browser and starts
  the same browser on the same profile with a visible window on that URL, in its own scope.
  Refused with 409 while a read is running, while a window is open, and with 400 for a
  URL that fails `check_url`. The owner passes the check or logs in and closes the window;
  when the browser exits, the host goes on the warmed-sites list
  (`<data>/reader-sites.json`: host and time). While the window is open, a read is
  `not done: the reader window is open` (the profile is in use).
- **Forget.** `DELETE /api/web/reader/sites/{host}` removes the host and clears its
  cookies from the profile through CDP (`Network.getAllCookies`, `Network.deleteCookies`
  for the domains that are the host or its parents' subdomain match). `DELETE
  /api/web/reader/profile` closes the browser (409 while a read runs or the window is
  open), deletes the profile folder and empties the list.
- `GET /api/web/reader`: `{enabled, browser, running, windowOpen, profile, warmed[]}`.
- **Search through the reader** is the last fallback of the chain (3.1): the page
  `https://html.duckduckgo.com/html/?q=<query>` read like any page (a challenge there is the
  same challenge and the same notice).

## 10. No disguise

Nothing in the reader hides that it is automated: no flags, scripts, injected
properties or user-agent changes meant to defeat a bot check (no `--disable-blink-features=
AutomationControlled`, no patched browser, no stealth script). A check that does not
clear is passed by the owner, once, in his own window (section 9), or the page is not
read. The decision of the owner (plan, section 5): no stealth; the reader has its own
profile; search through the reader uses DuckDuckGo.
