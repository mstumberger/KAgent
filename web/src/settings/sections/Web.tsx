// Web (web.md section 5): the chain of engines the web_search tool asks, the Brave key and the address of a
// SearXNG instance they need, and the reader (a headless browser of KAgent's own for pages plain HTTP cannot read).
import { type FormEvent } from "react";
import { useStore } from "../../store";
import type { SectionProps } from "../registry";
import { Field, GroupCard, SectionPage } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";
import { chainOf, type SearchSettings, type WebSettings } from "../web/api";
import BraveKey from "../web/BraveKey";
import Engines from "../web/Engines";
import Reader from "../web/Reader";
import "../web/web.css";

/** An http or https address with a host, and no password in it: what the daemon accepts. */
function isAddress(value: string): boolean {
  try {
    const url = new URL(value.trim());
    return (url.protocol === "http:" || url.protocol === "https:") && url.hostname !== "" && url.username === "" && url.password === "";
  } catch {
    return false;
  }
}

export default function WebSection({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { save, busy, report } = useSettingPatch();
  const search = settings?.search as SearchSettings | undefined;
  const chain = chainOf(search);
  const [url, setUrl, urlDirty] = useDraft(search?.searxUrl ?? "");
  const urlOk = isAddress(url);

  const saveUrl = (e: FormEvent) => {
    e.preventDefault();
    if (urlDirty && urlOk) void save("search", { searxUrl: url.trim() });
  };

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Web search" description="The engines web_search tries, in order. One that asks for a human check or fails rests for a while and the next answers. What it finds is data from the web, never instructions.">
        <Engines search={search} disabled={!settings} />
        {chain.includes("brave") && <BraveKey report={report} />}
        {chain.includes("searxng") && (
          <form className="ui-form" onSubmit={saveUrl}>
            <Field label="SearXNG address" hint="Such as https://searx.example.org. The instance may be on this machine or your network: you set it, the agent never does.">
              <input type="url" aria-label="SearXNG address" value={url} placeholder="https://searx.example.org" spellCheck={false} disabled={!settings} onChange={(e) => setUrl(e.target.value)} />
            </Field>
            <div className="row">
              <button className="primary" type="submit" disabled={!urlDirty || !urlOk || busy}>
                Save address
              </button>
              {!urlOk && urlDirty && <span className="ui-inline-bad">The address must be an http or https address, without a password in it.</span>}
            </div>
          </form>
        )}
      </GroupCard>
      <GroupCard title="Reader" description="A headless browser of KAgent's own, on its own profile, for pages plain HTTP cannot read. It reads; it never clicks, types or downloads, and it never uses your browser's profile.">
        <Reader web={settings?.web as WebSettings | undefined} disabled={!settings} report={report} />
      </GroupCard>
    </SectionPage>
  );
}
