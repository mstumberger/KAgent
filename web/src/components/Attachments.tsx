// The images waiting in the composer (vision.md section 1): 56 px thumbnails with a remove
// button each, and the line that says where images go when that is not this machine.
import type { Attachment } from "../images";
import Icon from "./Icon";
import "./images.css";

interface Props {
  items: Attachment[];
  onRemove: (key: string) => void;
  /** "Images are sent to X." until the session was told once; null: nothing to say. */
  providerLine: string | null;
}

export default function Attachments({ items, onRemove, providerLine }: Props) {
  if (items.length === 0) return null;
  return (
    <div className="pics" role="group" aria-label="Images to send">
      <ul className="pics-list">
        {items.map((a) => (
          <li key={a.key} className={`pics-item${a.ref ? "" : " busy"}`} data-name={a.name}>
            <img src={a.preview} alt={a.name} width={56} height={56} />
            <button type="button" className="pics-remove" aria-label={`Remove ${a.name}`} title="Remove" onClick={() => onRemove(a.key)}>
              <Icon name="close" size={12} />
            </button>
          </li>
        ))}
      </ul>
      {providerLine && <p className="pics-line">{providerLine}</p>}
    </div>
  );
}
