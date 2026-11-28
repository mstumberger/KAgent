// The line above the composer when a limit of the answering model's provider passes 90 % (usage.md section 7).
import { useEffect } from "react";
import { announce, dismiss, isOver, useLine } from "../usageLine";
import { providerOfRef, usageOf, useUsage } from "../usage";
import { useChoice } from "./useChoice";
import Icon from "./Icon";
import "./usage.css";

export default function UsageLine({ sessionId }: { sessionId: string | null }) {
  const choice = useChoice(sessionId);
  const usage = useUsage();
  const line = useLine();
  const provider = usageOf(usage, providerOfRef(choice.answering));
  const worst = provider?.tightest ?? null;
  useEffect(() => {
    if (provider && worst && isOver(worst)) announce(provider, worst);
  }, [provider, worst]);
  if (!line) return null;
  return (
    <div className="ul" role="status">
      <Icon name="alert" size={14} />
      <span className="ul-text">{line.text}</span>
      <button type="button" className="ul-close" aria-label="Dismiss" onClick={dismiss}>
        <Icon name="close" size={12} />
      </button>
    </div>
  );
}
