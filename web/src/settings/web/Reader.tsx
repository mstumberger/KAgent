// The reader (web.md sections 8 and 9): KAgent's own headless browser for pages that plain HTTP cannot read.
// On or off, the browser it found, the sites it is always used for, the sites the owner warmed (a human
// check passed once in a window on its profile) with Forget, and clearing the profile.
import { useEffect, useState, type FormEvent } from "react";
import { saveSettings } from "../useSettingPatch";
import { Badge, Confirm, EmptyState, Field, Row, Toggle } from "../ui";
import { webApi, type ReaderStatus, type WebSettings } from "./api";

const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));
/** What the daemon accepts as a site: lowercase letters, digits, dots and hyphens, with a dot, no scheme, port or path. */
const isHost = (v: string) => /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/.test(v);
/** What a pasted address or name becomes: a bare host. */
function hostOf(raw: string): string {
  const text = raw.trim().toLowerCase();
  try {
    return new URL(/^[a-z]+:\/\//.test(text) ? text : `https://${text}`).hostname;
  } catch {
    return text;
  }
}

export default function Reader({ web, disabled, report }: { web: WebSettings | undefined; disabled: boolean; report: (text: string, failed?: boolean) => void }) {
  const enabled = web?.reader ?? true;
  const sites = web?.readerSites ?? [];
  const [status, setStatus] = useState<ReaderStatus | null>(null);
  const [site, setSite] = useState("");
  const [confirm, setConfirm] = useState(false);
  const [working, setWorking] = useState(false);
  const refresh = () =>
    webApi.reader().then(
      (next) => setStatus(next),
      () => setStatus(null),
    );
  // The state of the reader, when the card shows and when the reader setting changes (the daemon counts it in `enabled`).
  useEffect(() => {
    void refresh();
  }, [enabled]);
  const act = async (what: string, run: () => Promise<void>) => {
    setWorking(true);
    try {
      await run();
    } catch (e) {
      report(`${what} failed: ${reason(e)}`, true);
    } finally {
      setWorking(false);
      void refresh();
    }
  };
  const host = hostOf(site);
  const addable = site.trim() !== "" && isHost(host) && !sites.includes(host);
  const add = (e: FormEvent) => {
    e.preventDefault();
    if (!addable) return;
    void saveSettings("web", { readerSites: [...sites, host] });
    setSite("");
  };
  return (
    <>
      <Row label="Use the reader" hint="When plain HTTP gets a human check or an empty page that needs JavaScript, web_fetch reads the page with a headless browser of its own, on its own profile. It never clicks, types or downloads.">
        <Toggle title="Use the reader" checked={enabled} disabled={disabled} onChange={(checked) => void saveSettings("web", { reader: checked })} />
      </Row>
      <Row label="Browser" hint="The first Chromium-family browser found on this machine. It is started when needed and closed after 2 idle minutes.">
        {status === null ? <span className="hint">No answer from the daemon.</span> : status.browser ? <code className="web-path">{status.browser}</code> : <Badge tone="warn">None found</Badge>}
      </Row>
      <div className="web-sites" data-kind="always">
        <h4>Always use the reader for</h4>
        <ul className="web-chips" aria-label="Sites always read by the reader">
          {sites.map((s) => (
            <li key={s} className="web-chip">
              <span>{s}</span>
              <button type="button" aria-label={`Stop using the reader for ${s}`} disabled={disabled} onClick={() => void saveSettings("web", { readerSites: sites.filter((x) => x !== s) })}>
                Remove
              </button>
            </li>
          ))}
        </ul>
        <form className="ui-form" onSubmit={add}>
          <Field label="Site" hint="A host such as news.example.com; its subdomains count too.">
            <input type="text" aria-label="Site for the reader" value={site} placeholder="news.example.com" spellCheck={false} disabled={disabled} onChange={(e) => setSite(e.target.value)} />
          </Field>
          <div className="row">
            <button className="primary" type="submit" disabled={disabled || !addable}>
              Add site
            </button>
            {site.trim() !== "" && !isHost(host) && <span className="ui-inline-bad">That is not a host name.</span>}
          </div>
        </form>
      </div>
      <div className="web-sites" data-kind="warmed">
        <h4>Warmed sites</h4>
        {status === null || status.warmed.length === 0 ? (
          <EmptyState title="No site is warmed" hint="When a site asks for a human check, a notice offers Open it: pass the check in the window, close it, and the next read of the site rides that session." />
        ) : (
          <ul className="web-warmed" aria-label="Warmed sites">
            {status.warmed.map((w) => (
              <li key={w.host} className="web-warmed-row">
                <span>{w.host}</span>
                <span className="hint">{new Date(w.at).toLocaleDateString()}</span>
                <button
                  type="button"
                  aria-label={`Forget ${w.host}`}
                  disabled={working}
                  onClick={() =>
                    void act("Forgetting the site", async () => {
                      await webApi.forget(w.host);
                      report(`${w.host} forgotten: its cookies are gone from the reader's profile.`);
                    })
                  }
                >
                  Forget
                </button>
              </li>
            ))}
          </ul>
        )}
        {status?.windowOpen && <p className="hint">The reader's window is open: close it to read pages again.</p>}
      </div>
      <div className="row">
        <button type="button" className="danger" disabled={working} onClick={() => setConfirm(true)}>
          Clear the reader's profile
        </button>
        {status && <span className="hint web-path">{status.profile}</span>}
      </div>
      {confirm && (
        <Confirm
          title="Clear the reader's profile?"
          body="Every cookie and login the reader kept is deleted, and the warmed sites are forgotten. Sites that asked for a human check will ask again."
          confirmLabel="Clear profile"
          danger
          onCancel={() => setConfirm(false)}
          onConfirm={() => {
            setConfirm(false);
            void act("Clearing the profile", async () => {
              await webApi.clearProfile();
              report("The reader's profile was cleared.");
            });
          }}
        />
      )}
    </>
  );
}
