// The Changes tab of a ticket: the diff of the branch or commit range the owner linked to it (diff.md section 6).
import { useEffect, useState } from "react";
import { api } from "../../api";
import DiffView from "../../components/diff/DiffView";
import type { Diff } from "../../components/diff/model";
import { errorMessage, type Link, type Ticket } from "./api";
import { rangeOf } from "./model";

interface Props {
  ticket: Ticket;
  onAddLink: (link: Link) => Promise<void>;
}

export default function TicketChanges({ ticket, onAddLink }: Props) {
  const range = rangeOf(ticket.links);
  const [diff, setDiff] = useState<Diff | null>(null);
  const [error, setError] = useState("");
  const [value, setValue] = useState("");
  const [kind, setKind] = useState<"range" | "commit">("range");
  const from = range?.from;
  const to = range?.to;
  useEffect(() => {
    if (!ticket.project || from === undefined) return;
    let live = true;
    api.post<{ diff: Diff }>("/api/diff", { project: ticket.project, from, ...(to ? { to } : {}) }).then(
      (r) => {
        if (!live) return;
        setDiff(r.diff);
        setError("");
      },
      (e) => {
        if (!live) return;
        setDiff(null);
        setError(errorMessage(e));
      },
    );
    return () => {
      live = false;
    };
  }, [ticket.project, from, to, ticket.updatedAt]);

  if (!ticket.project) return <div className="tk-changes tk-none">A ticket of the Inbox has no project, so there is no repository to compare.</div>;
  const add = () => {
    const v = value.trim();
    if (v) void onAddLink({ kind, value: v, label: "" }).then(() => setValue(""), (e) => setError(errorMessage(e)));
  };
  return (
    <div className="tk-changes">
      {!range && <p className="tk-none">Link a branch or commit range to see its changes.</p>}
      {range && <p className="tk-none">Comparing <code>{range.to ? `${range.from}..${range.to}` : `${range.from} and the working tree`}</code></p>}
      {error && <div className="tk-error" role="alert">{error}</div>}
      {diff && <DiffView diff={diff} label={`Changes of ${ticket.key}`} />}
      {!range && (
        <div className="tk-addlink">
          <select aria-label="Link kind for the changes" value={kind} onChange={(e) => setKind(e.target.value as "range" | "commit")}>
            <option value="range">Git range</option>
            <option value="commit">Commit</option>
          </select>
          <input
            type="text"
            aria-label="Range or commit to compare"
            placeholder={kind === "range" ? "main..feature" : "a commit id"}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                add();
              }
            }}
          />
          <button type="button" onClick={add}>Link</button>
        </div>
      )}
    </div>
  );
}
