// The card "Screen watch" of Desktop and apps (screen-watch.md section 4): whether KAgent may watch the
// screen when asked, which local model looks, and what it may do with what it sees.
import { useEffect, useState } from "react";
import { api } from "../../api";
import { setSection, showView, useStore } from "../../store";
import { GroupCard, Row, Toggle } from "../ui";
import { useSettingPatch } from "../useSettingPatch";

interface ModelView {
  model: string | null;
  server: string | null;
  refusal: string | null;
}

const MINUTES = [5, 10, 20, 30, 60, 120];
const EVERY = [5, 10, 15, 30, 60];

/** The standard choices, and the stored value when it is none of them. */
const withValue = (list: number[], value: number) => (list.includes(value) ? list : [...list, value].sort((a, b) => a - b));

export default function ScreenWatchCard() {
  const cfg = useStore().state?.settings?.screenWatch ?? {};
  const { save } = useSettingPatch();
  const [model, setModel] = useState<ModelView | null>(null);
  useEffect(() => {
    let live = true;
    void api
      .get<ModelView>("/api/screen-watch/model")
      .then((m) => live && setModel(m))
      .catch(() => live && setModel({ model: null, server: null, refusal: "KAgent could not ask which model it would use." }));
    return () => {
      live = false;
    };
  }, []);
  const minutes = cfg.defaultMinutes ?? 20;
  const every = cfg.everySec ?? 10;
  const put = (patch: Record<string, unknown>) => void save("screenWatch", patch);

  return (
    <GroupCard title="Screen watch" description={'Ask KAgent to keep an eye on a window or the monitor for a goal and a time ("tell me when the build finishes"). A local vision model looks every few seconds; no picture is stored, and none leaves this machine unless you allow it below. A chip in the title bar shows while it runs.'}>
      <Row label="Allow screen watch" hint="Off: KAgent refuses to start one. A watch that runs goes on until it ends or you stop it.">
        <Toggle title="Allow screen watch" checked={cfg.enabled ?? true} onChange={(v) => put({ enabled: v })} />
      </Row>
      <Row label="Model" hint="The vision model that looks. It must run on this machine or this network.">
        <div className="sw-model" data-state={model === null ? "loading" : model.model ? "ok" : "none"}>
          {model === null && <span className="hint">Looking...</span>}
          {model?.model && (
            <span>
              <code>{model.model}</code> <span className="hint">at {model.server}</span>
            </span>
          )}
          {model && !model.model && (
            <span>
              <span className="ui-inline-bad">{model.refusal}</span>{" "}
              <button
                className="link"
                onClick={() => {
                  showView("settings");
                  setSection("local-models");
                }}
              >
                Open Local models
              </button>
            </span>
          )}
        </div>
      </Row>
      <Row label="Default length" hint="How long a watch lasts when you do not say. At most 2 hours.">
        <select aria-label="Default length" value={minutes} onChange={(e) => put({ defaultMinutes: Number(e.target.value) })}>
          {withValue(MINUTES, minutes).map((m) => (
            <option key={m} value={m}>
              {m < 60 ? `${m} minutes` : `${m / 60} hour${m > 60 ? "s" : ""}`}
            </option>
          ))}
        </select>
      </Row>
      <Row label="Look every" hint="Seconds between looks. KAgent slows down when the model is slow.">
        <select aria-label="Look every" value={every} onChange={(e) => put({ everySec: Number(e.target.value) })}>
          {withValue(EVERY, every).map((s) => (
            <option key={s} value={s}>
              {s} seconds
            </option>
          ))}
        </select>
      </Row>
      <Row label="Confirm a hit with the cloud model" hint="When the local model says the goal is met, one frame goes to your cloud vision model to check. Off: nothing ever leaves this machine.">
        <Toggle title="Confirm a hit with the cloud model" checked={cfg.cloudConfirm ?? false} onChange={(v) => put({ cloudConfirm: v })} />
      </Row>
      <Row label="Also tell me on Telegram" hint="A hit and the end of a watch go to your paired chat too.">
        <Toggle title="Also tell me on Telegram" checked={cfg.telegram ?? false} onChange={(v) => put({ telegram: v })} />
      </Row>
    </GroupCard>
  );
}
