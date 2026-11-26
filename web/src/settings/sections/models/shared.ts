// What the four Models files share. A .ts file, so the component files export
// components only (fast refresh).
import type { ProviderView } from "../../../api";

export const API_FORMATS: Record<string, string> = {
  "anthropic-messages": "Anthropic messages (/v1/messages)",
  "openai-chat-completions": "Chat completions (/v1/chat/completions)",
  "openai-responses": "Responses (/v1/responses)",
  "ollama-native": "Ollama native (/api/chat)",
};

export type Status = "ok" | "error" | "nokey" | "off";

export const STATUS_LABEL: Record<Status, string> = { ok: "Connected", error: "Error", nokey: "No API key", off: "Disabled" };

/** Connected, in error, without a key (a remote provider), or switched off. */
export function statusOf(p: ProviderView): Status {
  if (!p.enabled) return "off";
  if (p.lastError) return "error";
  const local = p.api.type === "ollama-native" || p.api.baseUrl.includes("127.0.0.1") || p.api.baseUrl.includes("localhost");
  if (!p.hasKey && !local) return "nokey";
  return "ok";
}
