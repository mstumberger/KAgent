// The images of a message (vision.md section 1): at most 320 px wide, and a click opens the
// full image over the window until Escape or a click.
import { useEffect, useState } from "react";
import type { ImageRef } from "../api";
import { useImageUrl } from "../images";
import Icon from "./Icon";
import "./images.css";

function Thumb({ sessionId, image, onOpen }: { sessionId: string | null; image: ImageRef; onOpen: (url: string) => void }) {
  const url = useImageUrl(sessionId, image.id);
  return (
    <button type="button" className="msg-image" aria-label={`Open image ${image.width} by ${image.height}`} disabled={!url} onClick={() => url && onOpen(url)}>
      {url ? <img src={url} alt={image.description ?? "attached image"} style={{ width: Math.min(320, image.width) }} /> : <span className="msg-image-wait" style={{ width: Math.min(320, image.width) }} />}
    </button>
  );
}

export default function MessageImages({ sessionId, images }: { sessionId: string | null; images: ImageRef[] }) {
  const [open, setOpen] = useState<string | null>(null);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(null);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open]);
  return (
    <>
      <div className="msg-images">
        {images.map((image) => (
          <Thumb key={image.id} sessionId={sessionId} image={image} onOpen={setOpen} />
        ))}
      </div>
      {open && (
        <div className="lightbox" role="dialog" aria-modal="true" aria-label="Image" onClick={() => setOpen(null)}>
          <img src={open} alt="full size" />
          <button type="button" className="lightbox-close" aria-label="Close" onClick={() => setOpen(null)}>
            <Icon name="close" size={18} />
          </button>
        </div>
      )}
    </>
  );
}
