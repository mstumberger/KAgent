// The detail pane of a ticket: every field edited in place, notes, links and the history (tickets.md section 6).
import { useState, type ReactNode } from "react";
import Icon from "../../components/Icon";
import Markdown from "../../components/Markdown";
import { openSession } from "../../store";
import { errorMessage, type Change, type Link, type LinkKind, type Ticket, type TState, type Priority } from "./api";
import TicketChanges from "./TicketChanges";
import { PRIORITIES, PRIORITY_LABEL, STATES, STATE_LABEL } from "./model";
import Select from "../../settings/Select";

interface Props {
  ticket: Ticket;
  /** The tickets of the same project, for the parent select. */
  siblings: Ticket[];
  projectName: string;
  /** A change; it rejects with the daemon's message. */
  onChange: (key: string, change: Change) => Promise<void>;
  onDelete: (key: string) => Promise<void>;
  onClose: () => void;
  /** More content under the fields (the Changes tab of the diff view). */
  children?: ReactNode;
}

/** A text that is saved when the focus leaves it (Enter, for a one-line text). */
function Text({ id, label, value, multiline = false, rows = 4, onSave }: { id: string; label: string; value: string; multiline?: boolean; rows?: number; onSave: (v: string) => void }) {
  // The text being typed belongs to the value it started from: a new value (a save, a change elsewhere) replaces it.
  const [typed, setTyped] = useState({ base: value, text: value });
  const draft = typed.base === value ? typed.text : value;
  const setDraft = (text: string) => setTyped({ base: value, text });
  const commit = () => {
    if (draft !== value) onSave(draft);
  };
  return (
    <label className="tk-field" htmlFor={id}>
      <span>{label}</span>
      {multiline ? (
        <textarea id={id} rows={rows} value={draft} onChange={(e) => setDraft(e.target.value)} onBlur={commit} />
      ) : (
        <input
          id={id}
          type="text"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit();
            }
          }}
        />
      )}
    </label>
  );
}

const LINK_KINDS: { id: LinkKind; label: string; hint: string }[] = [
  { id: "range", label: "Git range", hint: "main..feature or a ref" },
  { id: "commit", label: "Commit", hint: "a commit id" },
  { id: "conversation", label: "Conversation", hint: "a session id" },
  { id: "url", label: "URL", hint: "https://..." },
];

export default function Detail({ ticket: t, siblings, projectName, onChange, onDelete, onClose, children }: Props) {
  const [error, setError] = useState("");
  const [confirm, setConfirm] = useState(false);
  const [label, setLabel] = useState("");
  const [note, setNote] = useState("");
  const [linkKind, setLinkKind] = useState<LinkKind>("range");
  const [linkValue, setLinkValue] = useState("");
  const [tab, setTab] = useState<"details" | "changes">("details");
  const save = async (change: Change) => {
    try {
      await onChange(t.key, change);
      setError("");
    } catch (e) {
      setError(errorMessage(e));
    }
  };
  const addLink = () => {
    const value = linkValue.trim();
    if (!value) return;
    void save({ links: [...t.links, { kind: linkKind, value, label: "" }] }).then(() => setLinkValue(""));
  };
  const open = (l: Link) => {
    if (l.kind === "conversation") void openSession(l.value);
    else if (l.kind === "url") window.open(l.value, "_blank", "noopener");
  };
  const parents = siblings.filter((s) => s.key !== t.key);
  return (
    <aside className={`tk-detail${tab === "changes" ? " wide" : ""}`} aria-label={`Ticket ${t.key}`} data-key={t.key}>
      <div className="tk-detail-head">
        <h3>{t.key}</h3>
        <span className="tk-project">{projectName}</span>
        <span className="spacer" />
        <button type="button" className="tk-icon" aria-label="Close the ticket" title="Close" onClick={onClose}>
          <Icon name="close" size={14} />
        </button>
      </div>
      <div className="tk-tabs" role="tablist" aria-label="Ticket">
        {(["details", "changes"] as const).map((x) => (
          <button key={x} type="button" role="tab" aria-selected={tab === x} onClick={() => setTab(x)}>
            {x === "details" ? "Details" : "Changes"}
          </button>
        ))}
      </div>
      {error && <div className="tk-error" role="alert">{error}</div>}
      {tab === "changes" ? (
        <TicketChanges ticket={t} onAddLink={(link) => onChange(t.key, { links: [...t.links, link] })} />
      ) : (
        <>
      <Text id="tk-title" label="Title" value={t.title} onSave={(title) => void save({ title })} />
      <div className="tk-grid">
        <label className="tk-field" htmlFor="tk-state">
          <span>State</span>
          <Select id="tk-state" value={t.state} onChange={(e) => void save({ state: e.target.value as TState })}>
            {STATES.map((s) => (
              <option key={s} value={s}>{STATE_LABEL[s]}</option>
            ))}
          </Select>
        </label>
        <label className="tk-field" htmlFor="tk-priority">
          <span>Priority</span>
          <Select id="tk-priority" value={t.priority} onChange={(e) => void save({ priority: e.target.value as Priority })}>
            {PRIORITIES.map((p) => (
              <option key={p} value={p}>{PRIORITY_LABEL[p]}</option>
            ))}
          </Select>
        </label>
        <label className="tk-field" htmlFor="tk-assignee">
          <span>Assignee</span>
          <Select id="tk-assignee" value={t.assignee ?? ""} onChange={(e) => void save({ assignee: e.target.value || null })}>
            <option value="">Nobody</option>
            <option value="owner">Me</option>
          </Select>
        </label>
        <label className="tk-field" htmlFor="tk-parent">
          <span>Parent</span>
          <Select id="tk-parent" value={t.parent ?? ""} onChange={(e) => void save({ parent: e.target.value || null })}>
            <option value="">None</option>
            {parents.map((p) => (
              <option key={p.key} value={p.key}>{p.key} {p.title.slice(0, 40)}</option>
            ))}
          </Select>
        </label>
      </div>
      <div className="tk-field">
        <span id="tk-labels-label">Labels</span>
        <div className="tk-chips" role="group" aria-labelledby="tk-labels-label">
          {t.labels.map((l) => (
            <span key={l} className="tk-label">
              {l}
              <button type="button" aria-label={`Remove label ${l}`} onClick={() => void save({ labels: t.labels.filter((x) => x !== l) })}>
                <Icon name="close" size={10} />
              </button>
            </span>
          ))}
          <input
            type="text"
            className="tk-inline"
            placeholder="Add a label"
            aria-label="Add a label"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && label.trim()) {
                e.preventDefault();
                void save({ labels: [...t.labels, label.trim()] }).then(() => setLabel(""));
              }
            }}
          />
        </div>
      </div>
      <Text id="tk-description" label="Description (markdown)" value={t.description} multiline rows={5} onSave={(description) => void save({ description })} />
      {t.description.trim() && (
        <div className="tk-preview" aria-label="Description preview">
          <Markdown text={t.description} />
        </div>
      )}
      <Text id="tk-done" label="Done when (one line each)" value={t.doneWhen.join("\n")} multiline rows={3} onSave={(v) => void save({ doneWhen: v.split("\n").map((l) => l.trim()).filter(Boolean) })} />
      <div className="tk-field">
        <span>Notes</span>
        {t.notes.map((n, i) => (
          <div key={i} className="tk-note"><span className="tk-by">{n.by}</span> {n.text}</div>
        ))}
        <input
          type="text"
          placeholder="Add a note"
          aria-label="Add a note"
          value={note}
          onChange={(e) => setNote(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && note.trim()) {
              e.preventDefault();
              void save({ note: note.trim() }).then(() => setNote(""));
            }
          }}
        />
      </div>
      <div className="tk-field">
        <span>Links</span>
        {t.links.map((l) => (
          <div key={`${l.kind}:${l.value}`} className="tk-link" data-kind={l.kind}>
            <span className="tk-by">{l.kind}</span>
            {l.kind === "conversation" || l.kind === "url" ? (
              <button type="button" className="tk-linkbtn" onClick={() => open(l)}>{l.value}</button>
            ) : (
              <code>{l.value}</code>
            )}
            <button type="button" className="tk-icon" aria-label={`Remove link ${l.value}`} onClick={() => void save({ links: t.links.filter((x) => x !== l) })}>
              <Icon name="close" size={12} />
            </button>
          </div>
        ))}
        <div className="tk-addlink">
          <Select aria-label="Link kind" value={linkKind} onChange={(e) => setLinkKind(e.target.value as LinkKind)}>
            {LINK_KINDS.map((k) => (
              <option key={k.id} value={k.id}>{k.label}</option>
            ))}
          </Select>
          <input
            type="text"
            aria-label="Link value"
            placeholder={LINK_KINDS.find((k) => k.id === linkKind)?.hint}
            value={linkValue}
            onChange={(e) => setLinkValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                addLink();
              }
            }}
          />
          <button type="button" onClick={addLink}>Add link</button>
        </div>
      </div>
      {children}
      <div className="tk-field">
        <span>History</span>
        <ol className="tk-history">
          {[...t.history].reverse().map((h, i) => (
            <li key={i}>
              <span className="tk-by">{h.by}</span> {h.what} <time dateTime={h.at}>{new Date(h.at).toLocaleString()}</time>
            </li>
          ))}
        </ol>
      </div>
        </>
      )}
      <div className="tk-actions">
        {confirm ? (
          <>
            <span>Delete {t.key}?</span>
            <button type="button" className="danger" onClick={() => void onDelete(t.key).catch((e) => setError(errorMessage(e)))}>Delete</button>
            <button type="button" onClick={() => setConfirm(false)}>Cancel</button>
          </>
        ) : (
          <button type="button" className="danger" onClick={() => setConfirm(true)}>
            <Icon name="trash" size={13} /> Delete ticket
          </button>
        )}
      </div>
    </aside>
  );
}
