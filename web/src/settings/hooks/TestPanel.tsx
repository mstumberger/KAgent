// The test panel of a hook (hooks.md section 7): the event whose payload is sent, the
// sample JSON (the documented example by default), Run, and the result. A test reaches
// the hook and the caller only, never the agent.
import { useState } from "react";
import { Badge } from "../ui";
import { EVENTS, hooksApi, type Hook, type HookEventName, type TestResult } from "./api";

interface Props {
  hook: Hook;
  /** The payload of each event as the daemon documents it. */
  samples: Partial<Record<HookEventName, object>>;
  onClose: () => void;
}

const why = (e: unknown) => (e instanceof Error ? e.message : String(e));

const sampleText = (samples: Props["samples"], event: HookEventName) => `${JSON.stringify(samples[event] ?? { event }, null, 2)}\n`;

export default function TestPanel({ hook, samples, onClose }: Props) {
  const [event, setEvent] = useState<HookEventName>(hook.event);
  const [sample, setSample] = useState(() => sampleText(samples, hook.event));
  const [result, setResult] = useState<TestResult | null>(null);
  const [failure, setFailure] = useState("");
  const [busy, setBusy] = useState(false);

  const pick = (next: HookEventName) => {
    setEvent(next);
    setSample(sampleText(samples, next));
    setResult(null);
    setFailure("");
  };
  const run = async () => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(sample);
    } catch (e) {
      setFailure(`The sample is not JSON: ${why(e)}`);
      return;
    }
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      setFailure("The sample must be one JSON object.");
      return;
    }
    setBusy(true);
    setFailure("");
    try {
      setResult(await hooksApi.test(hook.id ?? "", { event, sample: parsed }));
    } catch (e) {
      setResult(null);
      setFailure(why(e));
    } finally {
      setBusy(false);
    }
  };
  const info = EVENTS.find((e) => e.name === event);
  const decision: string = result ? (result.error ? "Failed" : result.decision ? `Decision: ${result.decision}` : "No decision: the agent goes on") : "";
  const tone = !result ? "neutral" : result.error ? "bad" : result.decision === "deny" ? "bad" : result.decision === "ask" ? "warn" : "ok";
  const output = (name: string, text: string) => (
    <div className="ui-field">
      <span className="ui-field-label">{name}</span>
      <pre className="hk-output">{text === "" ? "(empty)" : text}</pre>
    </div>
  );

  return (
    <div className="hk-test" data-hook-id={hook.id}>
      <p className="hint">
        Runs the hook as it is stored, with the payload below on its stdin. The answer is shown here and goes nowhere else.
      </p>
      <label className="ui-field">
        <span className="ui-field-label">Event of the sample</span>
        <select value={event} aria-label="Event of the sample" onChange={(e) => pick(e.target.value as HookEventName)}>
          {EVENTS.map((e) => (
            <option key={e.name} value={e.name}>
              {e.name}
            </option>
          ))}
        </select>
        <span className="hint">{info?.when}</span>
      </label>
      <label className="ui-field">
        <span className="ui-field-label">Payload (JSON)</span>
        <textarea className="hk-mono hk-sample" rows={10} value={sample} aria-label="Sample payload" spellCheck={false} onChange={(e) => setSample(e.target.value)} />
      </label>
      <div className="row">
        <button className="primary" type="button" disabled={busy} onClick={() => void run()}>
          Run
        </button>
        <button type="button" disabled={busy} onClick={onClose}>
          Close
        </button>
      </div>
      {failure && (
        <p className="hk-error" role="alert">
          {failure}
        </p>
      )}
      {result && (
        <div className="hk-result" role="status">
          <div className="hk-result-head">
            <Badge tone={tone}>{decision}</Badge>
            <span>exit code {result.exitCode === null ? "none" : result.exitCode}</span>
            <span>{result.durationMs} ms</span>
            {result.timedOut && <Badge tone="bad">timed out</Badge>}
          </div>
          {result.error && <p className="hk-error">{result.error}</p>}
          {output("stdout", result.stdout)}
          {output("stderr", result.stderr)}
        </div>
      )}
    </div>
  );
}
