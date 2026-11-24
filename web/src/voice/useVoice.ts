import { useSyncExternalStore } from "react";
import { voice } from "./controller";

/** Re-render on every voice change (status, transcript, mute, volume). */
export function useVoice() {
  useSyncExternalStore(voice.subscribe, () => voice.version);
  return voice;
}
