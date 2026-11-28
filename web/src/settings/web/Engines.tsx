// The chain of search engines (web.md section 3.1): the engines web_search tries, in order. Each has a
// switch; the ones that are on can be moved. An engine that fails goes on a cooldown and the next one answers.
import { saveSettings } from "../useSettingPatch";
import { Toggle } from "../ui";
import { CHAIN_ENGINES, chainOf, type SearchEngine, type SearchSettings } from "./api";

type Id = Exclude<SearchEngine, "none">;

const ENGINES: Record<Id, { label: string; hint: string }> = {
  duckduckgo: { label: "DuckDuckGo", hint: "No key. It limits a client that sends many searches: the tool waits between searches and moves on when it is asked for a human check." },
  brave: { label: "Brave Search API", hint: "Needs the key of api.search.brave.com. It stays in the daemon and your desktop keyring." },
  searxng: { label: "SearXNG", hint: "Your own instance, or one you trust. The instance must allow its JSON format." },
  reader: { label: "The reader", hint: "DuckDuckGo's result page read by KAgent's own headless browser. Slow, and the last resort." },
};

export default function Engines({ search, disabled }: { search: SearchSettings | undefined; disabled: boolean }) {
  const on = chainOf(search);
  const off = CHAIN_ENGINES.filter((e) => !on.includes(e));
  const save = (next: Id[]) => void saveSettings("search", { engines: next });
  const move = (id: Id, by: -1 | 1) => {
    const at = on.indexOf(id);
    const next = [...on];
    next.splice(at, 1);
    next.splice(at + by, 0, id);
    save(next);
  };
  const row = (id: Id, at: number | null) => (
    <li key={id} className="web-engine" data-engine={id} data-on={at !== null ? "true" : "false"}>
      <Toggle title={`Use ${ENGINES[id].label}`} checked={at !== null} disabled={disabled} onChange={(checked) => save(checked ? [...on, id] : on.filter((e) => e !== id))} />
      <span className="web-engine-text">
        <span className="web-radio-label">{ENGINES[id].label}</span>
        <span className="hint">{ENGINES[id].hint}</span>
      </span>
      {at !== null && (
        <span className="web-engine-move">
          <button type="button" aria-label={`Move ${ENGINES[id].label} up`} disabled={disabled || at === 0} onClick={() => move(id, -1)}>
            Up
          </button>
          <button type="button" aria-label={`Move ${ENGINES[id].label} down`} disabled={disabled || at === on.length - 1} onClick={() => move(id, 1)}>
            Down
          </button>
        </span>
      )}
    </li>
  );
  return (
    <>
      <ol className="web-engines" aria-label="Search engines, in the order they are tried">
        {on.map((id, at) => row(id, at))}
        {off.map((id) => row(id, null))}
      </ol>
      {on.length === 0 && <p className="hint web-off">All engines are off: web_search answers that search is off. web_fetch keeps working.</p>}
    </>
  );
}
