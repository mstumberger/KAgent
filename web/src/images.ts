// Images of a message (vision.md section 1): the limits the composer checks before it uploads
// (the daemon checks them again), the upload, and the stored image as an object URL (the
// route needs the bearer token, which an <img> cannot send).
import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, type ImageRef } from "./api";

export const MAX_IMAGES = 4;
export const MAX_BYTES = 8 * 1024 * 1024;
export const IMAGE_TYPES = ["image/png", "image/jpeg", "image/webp"];
export const ACCEPT = IMAGE_TYPES.join(",");

/** Why a file cannot be attached, or null. */
export function refusal(file: File, already: number): string | null {
  if (already >= MAX_IMAGES) return `A message takes at most ${MAX_IMAGES} images: remove one first.`;
  if (!IMAGE_TYPES.includes(file.type)) return "Only PNG, JPEG and WebP images can be attached.";
  if (file.size > MAX_BYTES) return "That image is over 8 MB: send a smaller one.";
  return null;
}

/** The images in a clipboard or drop transfer. */
export function imageFiles(list: FileList | DataTransferItemList | null | undefined): File[] {
  const out: File[] = [];
  if (!list) return out;
  for (let i = 0; i < list.length; i += 1) {
    const entry = list[i];
    const file = entry instanceof File ? entry : entry.kind === "file" ? entry.getAsFile() : null;
    if (file && file.type.startsWith("image/")) out.push(file);
  }
  return out;
}

function token(): string {
  return localStorage.getItem("kagent.token") ?? "";
}

/** `POST /api/sessions/:id/images`: one file; the answer is its reference. */
export async function uploadImage(sessionId: string, file: File): Promise<ImageRef> {
  const form = new FormData();
  form.append("file", file, file.name || "image");
  const res = await fetch(`/api/sessions/${sessionId}/images`, { method: "POST", headers: { Authorization: `Bearer ${token()}` }, body: form });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new ApiError(res.status, data?.error?.code ?? "unknown", data?.error?.message ?? res.statusText);
  return data as ImageRef;
}

const urls = new Map<string, Promise<string>>();

/** The stored image as an object URL, fetched once. */
export function imageUrl(sessionId: string, imageId: string): Promise<string> {
  const key = `${sessionId}/${imageId}`;
  let url = urls.get(key);
  if (!url) {
    url = fetch(`/api/sessions/${sessionId}/images/${imageId}`, { headers: { Authorization: `Bearer ${token()}` } }).then(async (res) => {
      if (!res.ok) throw new Error(`image ${res.status}`);
      return URL.createObjectURL(await res.blob());
    });
    urls.set(key, url);
    url.catch(() => urls.delete(key));
  }
  return url;
}

export function useImageUrl(sessionId: string | null, imageId: string): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!sessionId) return;
    let live = true;
    imageUrl(sessionId, imageId).then(
      (u) => live && setUrl(u),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [sessionId, imageId]);
  return url;
}

/** One image of the composer: shown at once from the file, sendable when `ref` is set. */
export interface Attachment {
  key: string;
  name: string;
  preview: string;
  ref?: ImageRef;
  failed?: string;
}

/** The images waiting in the composer of a session: added, uploaded one by one, removed, taken for a send. */
export function useAttachments(sessionId: string | null, onRefused: (text: string) => void) {
  const [items, setItems] = useState<Attachment[]>([]);
  const owner = useRef<string | null>(sessionId);
  const count = useRef(0);
  // A conversation of its own: what waited in another one stays out of this one.
  useEffect(() => {
    if (owner.current !== sessionId) {
      owner.current = sessionId;
      setItems([]);
    }
  }, [sessionId]);

  const add = useCallback(
    (files: File[]) => {
      if (!sessionId) return;
      let have = items.length;
      for (const file of files) {
        const why = refusal(file, have);
        if (why) {
          onRefused(why);
          continue;
        }
        have += 1;
        const key = `a${(count.current += 1)}`;
        const entry: Attachment = { key, name: file.name || "image", preview: URL.createObjectURL(file) };
        setItems((cur) => [...cur, entry]);
        uploadImage(sessionId, file).then(
          (ref) => setItems((cur) => cur.map((a) => (a.key === key ? { ...a, ref } : a))),
          (e) => {
            setItems((cur) => cur.filter((a) => a.key !== key));
            onRefused(e instanceof Error ? e.message : String(e));
          },
        );
      }
    },
    [sessionId, items.length, onRefused],
  );
  const remove = useCallback((key: string) => setItems((cur) => cur.filter((a) => a.key !== key)), []);
  const clear = useCallback(() => setItems([]), []);
  const restore = useCallback((back: Attachment[]) => setItems(back), []);
  return { items, add, remove, clear, restore, uploading: items.some((a) => !a.ref) };
}

/** `http://127.0.0.1:11434/v1` and `localhost` are on this machine. */
export function isLocalUrl(baseUrl: string): boolean {
  try {
    const host = new URL(baseUrl).hostname;
    return host === "localhost" || host === "::1" || host === "[::1]" || host.startsWith("127.");
  } catch {
    return false;
  }
}

/** Sessions that were told where their images go (vision.md section 2): once each. */
const told = new Set<string>();
export const providerLineTold = (sessionId: string) => told.has(sessionId);
export const tellProviderLine = (sessionId: string) => void told.add(sessionId);
