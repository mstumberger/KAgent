// Telegram (bots.md section 7): talk to KAgent from a phone, and get alerts, approvals
// and automation results there. The daemon only makes an outbound connection to
// Telegram; the bot token stays in the daemon, and this page only hears whether there
// is one.
import { useEffect, useState, type FormEvent } from "react";
import { events } from "../../events";
import { applySettings, useStore } from "../../store";
import type { SectionProps } from "../registry";
import { Badge, Field, GroupCard, Row, SectionPage, Toggle, type BadgeTone } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";
import { telegramApi, type TelegramSettings, type TelegramStatus } from "../telegram/api";
import "../telegram/telegram.css";

const DEFAULT_DELAY = 20;
/** The line about remote control (bots.md section 7). */
const WARNING = "Whoever can write in this Telegram chat can ask KAgent to do things. Approvals follow the permission mode.";

/** The badge of the status card for the state of the bot. */
function stateOf(status: TelegramStatus | null): { tone: BadgeTone; text: string } {
  if (!status) return { tone: "neutral", text: "No answer from the daemon" };
  switch (status.status) {
    case "off":
      return { tone: "neutral", text: "Not configured" };
    case "disabled":
      return { tone: "neutral", text: "Off" };
    case "unpaired":
      return { tone: "warn", text: "Waiting for pairing" };
    case "polling":
      return { tone: "ok", text: "Connected" };
    default:
      return { tone: "bad", text: "Error" };
  }
}

function when(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

/** When a pairing code stops being good, in words. */
function expiry(iso: string): string {
  const minutes = Math.ceil((new Date(iso).getTime() - Date.now()) / 60000);
  if (minutes <= 0) return "expired; ask for a new code";
  return `valid for ${minutes} minute${minutes === 1 ? "" : "s"}, until ${when(iso)}`;
}

const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));
const bot = (name: string | null | undefined) => (name ? `@${name}` : "the bot");

export default function TelegramSection({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { report, busy } = useSettingPatch();
  const [status, setStatus] = useState<TelegramStatus | null>(null);
  const [token, setToken] = useState("");
  const [working, setWorking] = useState(false);
  const telegram = settings?.telegram ?? {};
  const [delay, setDelay, delayDirty] = useDraft(telegram.approvalForwardSec ?? DEFAULT_DELAY);
  const delayOk = Number.isInteger(delay) && delay >= 5 && delay <= 600;
  const state = stateOf(status);
  const on = telegram.enabled === true;
  const configured = status?.configured === true;
  const paired = status?.paired === true;
  const name = status?.botUsername ?? null;
  const code = status?.pendingCode;

  // The status when the page opens, and again after every telegramStatus event (bots.md
  // section 6): the event names the state, the answer adds the chat and the code.
  useEffect(() => {
    let open = true;
    const read = () =>
      telegramApi.status().then(
        (next) => {
          if (open) setStatus(next);
        },
        () => {},
      );
    read();
    const off = events.on((ev) => {
      if (ev.type === "telegramStatus") void read();
    });
    return () => {
      open = false;
      off();
    };
  }, []);

  /** One change of the settings: one PUT /api/telegram; the daemon then tells every UI. */
  const save = async (patch: TelegramSettings) => {
    try {
      setStatus(await telegramApi.save(patch));
      // The same settings the daemon announces next, for while the event stream is down.
      if (settings) applySettings({ ...settings, telegram: { ...telegram, ...patch } });
      report("Saved");
    } catch (e) {
      report(`Not saved: ${reason(e)}`, true);
    }
  };

  /** One action of the page: its result goes to the header, a failure too. */
  const act = async (what: string, run: () => Promise<void>) => {
    setWorking(true);
    try {
      await run();
    } catch (e) {
      report(`${what} failed: ${reason(e)}`, true);
    } finally {
      setWorking(false);
    }
  };
  const refresh = async () => setStatus(await telegramApi.status());
  const showCode = async () => {
    await telegramApi.pairingCode();
    await refresh();
    report("Pairing code shown. Send it to the bot from your phone.");
  };
  const unpair = async () => {
    await telegramApi.unpair();
    await refresh();
    report("Unpaired. The chat is forgotten.");
  };
  const testMessage = async () => {
    const result = await telegramApi.test();
    if (!result.ok) throw new Error(result.error ?? "Telegram did not take it");
    report("Sent. Look at the Telegram chat.");
  };
  const deleteToken = async () => {
    await telegramApi.deleteToken();
    await refresh();
    report("Token deleted. The chat is forgotten.");
  };
  const saveToken = (e: FormEvent) => {
    e.preventDefault();
    const value = token.trim();
    if (!value) return;
    void act("Saving the token", async () => {
      const saved = await telegramApi.setToken(value);
      setToken("");
      await refresh();
      report(`Saved. The bot is @${saved.botUsername}.`);
    });
  };
  const saveDelay = () => {
    if (delayDirty && delayOk) void save({ approvalForwardSec: delay });
  };

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Telegram bot" description="KAgent opens no port for this: the bot connects to Telegram and asks for messages.">
        <Row label="Telegram bot" hint="Telegram's servers see every message exchanged.">
          <Toggle title="Telegram bot" checked={on} disabled={!settings} onChange={(checked) => void save({ enabled: checked })} />
        </Row>
        <div className="tg-status" data-status={status?.status ?? "none"}>
          <div className="tg-line">
            <Badge tone={state.tone}>{state.text}</Badge>
            {name && <span className="tg-name">@{name}</span>}
            {paired && status.chatId !== null && <span className="hint">Chat {status.chatId}, paired since {when(status.pairedAt ?? "")}</span>}
          </div>
          {status?.status === "error" && <p className="hint tg-bad">{status.lastError ?? "The last poll failed."}</p>}
          {status?.status === "polling" && status.lastError && <p className="hint">Last error, since recovered: {status.lastError}</p>}
          {status?.status === "unpaired" && <p className="hint">Ask for a pairing code, then send it as /start &lt;code&gt; to {bot(name)} from the Telegram chat that should talk to KAgent.</p>}
          {status?.status === "disabled" && <p className="hint">Switched off. Turn the Telegram bot on to pair a chat.</p>}
          {status?.status === "off" && <p className="hint">Not configured yet: paste the token of a bot you made with @BotFather.</p>}
          {code && (
            <p className="tg-code">
              <span className="tg-code-digits">{code.code}</span>
              <span className="hint">Pairing code, {expiry(code.expiresAt)}.</span>
              <span className="hint">
                Send <code>/start {code.code}</code> to {bot(name)}.
              </span>
            </p>
          )}
          <div className="row">
            <button type="button" disabled={working || !configured || !on || paired} onClick={() => void act("Pairing code", showCode)}>
              Show pairing code
            </button>
            {paired && (
              <button type="button" disabled={working} onClick={() => void act("Unpairing", unpair)}>
                Unpair
              </button>
            )}
            <button type="button" disabled={working || !paired || !on} onClick={() => void act("Test message", testMessage)}>
              Send test message
            </button>
            {configured && (
              <button type="button" className="danger" disabled={working} onClick={() => void act("Deleting the token", deleteToken)}>
                Delete token
              </button>
            )}
          </div>
        </div>
        <form className="ui-form" onSubmit={saveToken}>
          <Field label="Bot token" hint="From @BotFather in Telegram. It stays in the daemon and your desktop keyring; this page only hears whether there is one.">
            <input type="password" value={token} autoComplete="off" spellCheck={false} placeholder={configured ? "A token is saved; paste another to replace it" : "123456789:AA..."} onChange={(e) => setToken(e.target.value)} />
          </Field>
          <div className="row">
            <button className="primary" type="submit" disabled={working || busy || token.trim() === ""}>
              Save token
            </button>
          </div>
        </form>
      </GroupCard>
      <GroupCard title="What goes to the chat" description="Alerts, approvals and the results of automations, each while the bot is on and a chat is paired.">
        <Row label="Push alerts" hint="New alerts of severity review and high. Review waits for the quiet hours to end; high always goes.">
          <Toggle title="Push alerts" checked={telegram.notifyAlerts !== false} disabled={!settings} onChange={(checked) => void save({ notifyAlerts: checked })} />
        </Row>
        <Row label="Forward approvals" hint="A card nobody answers on the desktop for a while goes to the chat, with Allow once and Deny. Destructive calls and plans are never sent.">
          <Toggle title="Forward approvals" checked={telegram.forwardApprovals === true} disabled={!settings} onChange={(checked) => void save({ forwardApprovals: checked })} />
        </Row>
        <form className="ui-form" onSubmit={(e) => void (e.preventDefault(), saveDelay())}>
          <Field label="Wait before forwarding" hint="5 to 600 seconds.">
            <input type="number" min={5} max={600} value={Number.isNaN(delay) ? "" : delay} disabled={!settings || telegram.forwardApprovals !== true} onChange={(e) => setDelay(e.target.valueAsNumber)} onBlur={saveDelay} />
          </Field>
        </form>
        <Row label="Allow root actions" hint="Root actions may be approved from the chat too. Off: their cards stay on the desktop.">
          <Toggle title="Allow root actions" checked={telegram.allowPrivileged === true} disabled={!settings} onChange={(checked) => void save({ allowPrivileged: checked })} />
        </Row>
        <p className="hint ui-note">{WARNING}</p>
      </GroupCard>
    </SectionPage>
  );
}
