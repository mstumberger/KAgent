// The images of the composer of the open conversation (vision.md section 1) with the line that
// says where they go (section 2): once per session, and only when that is not this machine.
import { useEffect, useState } from "react";
import { api, type ProviderView } from "../api";
import { isLocalUrl, providerLineTold, useAttachments } from "../images";
import { notify } from "./toasts";

/** The provider an image of this conversation goes to: the Brain's when it reads images, else the vision model's. */
export function imageProvider(answering: string, models: { ref: string; config: { supportsImage: boolean } }[] | null, visionModel: string): string | null {
  const brain = (models ?? []).find((m) => m.ref === answering);
  const ref = brain?.config.supportsImage ? answering : visionModel;
  return ref ? ref.split("::")[0] : null;
}

export function useComposerImages(sessionId: string | null, answering: string, models: { ref: string; config: { supportsImage: boolean } }[] | null, visionModel: string) {
  const attachments = useAttachments(sessionId, (text) => notify(text));
  const provider = imageProvider(answering, models, visionModel);
  // What the providers said for one provider; the line itself is derived while rendering.
  const [said, setSaid] = useState<{ provider: string; text: string | null } | null>(null);
  const waiting = attachments.items.length > 0 && !!sessionId && !providerLineTold(sessionId);
  useEffect(() => {
    if (!waiting || !provider) return;
    let live = true;
    api.get<{ providers: ProviderView[] }>("/api/providers").then(
      (r) => {
        const p = r.providers.find((x) => x.id === provider);
        if (live) setSaid({ provider, text: p && !isLocalUrl(p.api.baseUrl) ? `Images are sent to ${p.name}.` : null });
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [waiting, provider]);
  return { ...attachments, providerLine: waiting && said?.provider === provider ? said.text : null };
}
